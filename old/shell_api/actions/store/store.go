package actions_space

import (
	"encoding/json"
	"errors"
	"kasper/src/abstract/models/action"
	"kasper/src/abstract/models/core"
	"kasper/src/abstract/models/trx"
	"kasper/src/abstract/state"
	inputs_stores "kasper/src/shell/api/inputs/stores"
	"kasper/src/shell/api/model"
	outputs_stores "kasper/src/shell/api/outputs/stores"
	updates_stores "kasper/src/shell/api/updates/stores"
	"kasper/src/shell/utils/future"
	"log"
	"maps"
	"os"
	"slices"
	"strings"
	"sync"
	"time"

	cmap "github.com/orcaman/concurrent-map/v2"
)

type LockHolder struct {
	Lock sync.Mutex
}

type Actions struct {
	App           core.ICore
	Locks         cmap.ConcurrentMap[string, *LockHolder]
	OneToOneLocks cmap.ConcurrentMap[string, *LockHolder]
	modelExtender map[string]map[string]action.ExtendedField
}

func putAccessLinks(tx trx.ITrx, creatureId string, storeId string) {
	tx.PutLink("hasaccess::"+creatureId+"::"+storeId, "true")
	tx.PutLink("onaccess::"+storeId+"::"+creatureId, "true")
}

func delAccessLinks(tx trx.ITrx, creatureId string, storeId string) {
	tx.DelKey("link::hasaccess::" + creatureId + "::" + storeId)
	tx.DelKey("link::onaccess::" + storeId + "::" + creatureId)
}

func Install(a *Actions, params ...any) error {
	a.Locks = cmap.New[*LockHolder]()
	a.OneToOneLocks = cmap.New[*LockHolder]()
	if len(params) >= 1 {
		a.modelExtender = params[0].(map[string]map[string]action.ExtendedField)
	} else {
		a.modelExtender = map[string]map[string]action.ExtendedField{}
	}
	if _, ok := a.modelExtender["user"]; !ok {
		a.modelExtender["user"] = map[string]action.ExtendedField{}
	}
	return nil
}

type Access struct {
	Name string `json:"name"`
}

var access = map[string]bool{
	"createSubStore": false,
	"deleteSubStore": false,
	"uploadEntity":   false,
	"updateMetadata": true,
	"sendSignal":     true,
	"readHistory":    true,
	"addMachine":     true,
	"addProgram":     true,
	"updateProgram":  true,
	"removeProgram":  true,
	"removeMachine":  true,
	"addMember":      false,
	"updateMember":   false,
	"readMembers":    false,
	"removeMember":   false,
}

func isGodUser(trx trx.ITrx, userId string, gods []string) bool {
	username := string(trx.GetColumn("User", userId, "username"))
	if username == "" {
		return false
	}
	return slices.Contains(gods, username)
}

func extractCommandPacket(raw string) (string, map[string]any, bool) {
	var body map[string]any
	if err := json.Unmarshal([]byte(raw), &body); err != nil {
		return "", nil, false
	}
	cmd, ok := body["command"].(string)
	if !ok || cmd == "" {
		return "", nil, false
	}
	params, ok := body["params"].(map[string]any)
	if !ok {
		params = map[string]any{}
	}
	return cmd, params, true
}

func (a *Actions) handleGodSignalCommand(state state.IState, input inputs_stores.SignalInput) (bool, any, error) {
	if input.Type != "single" || input.UserId != "god@"+a.App.Id() || !isGodUser(state.Trx(), state.Info().UserId(), a.App.Gods()) {
		return false, nil, nil
	}
	command, params, ok := extractCommandPacket(input.Data)
	if !ok {
		return true, outputs_stores.SignalOutput{Passed: false}, nil
	}
	switch command {
	case "shutdown":
		go func() {
			time.Sleep(100 * time.Millisecond)
			os.Exit(0)
		}()
		return true, outputs_stores.SignalOutput{Passed: true}, nil
	case "assignGod":
		username, _ := params["username"].(string)
		if username != "" {
			targetId := state.Trx().GetIndex("User", "username", "id", username)
			if targetId != "" {
				state.Trx().PutString("god::"+targetId, "true")
				a.App.AddGod(username)
				return true, outputs_stores.SignalOutput{Passed: true}, nil
			}
		}
		return true, outputs_stores.SignalOutput{Passed: false}, nil
	case "assignFreeNode":
		nodeId, _ := params["nodeId"].(string)
		if nodeId != "" {
			a.App.AddFreeNode(nodeId)
			return true, outputs_stores.SignalOutput{Passed: true}, nil
		}
		return true, outputs_stores.SignalOutput{Passed: false}, nil
	default:
		return true, outputs_stores.SignalOutput{Passed: false}, nil
	}
}

// AddMachine /stores/addMachine check [ true true false ] access [ true false false false POST ]
func (a *Actions) AddMachine(state state.IState, input inputs_stores.AddMachineInput) (any, error) {
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		if meta, err := state.Trx().GetJson("StoreAccess::"+state.Info().StoreId()+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["addMachine"].(bool) {
			return nil, errors.New("access not permitted")
		}
	}
	trx := state.Trx()
	a.Locks.SetIfAbsent(state.Info().StoreId(), &LockHolder{})
	locker, _ := a.Locks.Get(state.Info().StoreId())
	locker.Lock.Lock()
	defer locker.Lock.Unlock()
	if !trx.HasObj("App", input.MachineId) {
		return nil, errors.New("machine not found")
	}
	programModel := model.Machine{Id: input.MachineId}.Pull(trx)

	programs, err := model.User{}.List(trx, "appMachines::"+input.MachineId+"::", map[string]string{})
	if err != nil {
		log.Println(err)
		return nil, err
	}
	programModels, err := model.Program{}.List(trx, "appMachines::"+input.MachineId+"::")
	if err != nil {
		log.Println(err)
		return nil, err
	}
	programMap := map[string]model.Program{}
	for _, program := range programModels {
		programMap[program.MachineId] = program
	}
	macMap := map[string]model.User{}
	for _, mac := range programs {
		macMap[mac.Id] = mac
	}
	m := map[string]*updates_stores.Fn{}
	uniqueMacs := map[string][]string{}
	for _, programMeta := range input.ProgramsMeta {

		mac := macMap[programMeta.ProgramId]
		program := programMap[programMeta.ProgramId]
		fn := &updates_stores.Fn{
			UserId:     mac.Id,
			Typ:        mac.Typ,
			Username:   mac.Username,
			PublicKey:  mac.PublicKey,
			ProgramId:  program.AppId,
			Runtime:    program.Runtime,
			Path:       program.Path,
			Comment:    program.Comment,
			Metadata:   programMeta.Metadata,
			Identifier: programMeta.Identifier,
			Access:     programMeta.Access,
		}
		m[fn.UserId+"::"+fn.Identifier] = fn
		acc := map[string]bool{}
		for k, v := range access {
			if v2, ok := programMeta.Access[k]; ok {
				acc[k] = v2
			} else {
				acc[k] = v
			}
		}
		trx.PutJson("StoreAccess::"+state.Info().StoreId()+"::"+fn.UserId, "metadata", acc, false)
		trx.PutJson("FnMeta::"+state.Info().StoreId()+"::"+fn.ProgramId+"::"+fn.UserId+"::"+programMeta.Identifier, "metadata", programMeta.Metadata, true)
		trx.PutLink("storeMachineProgram::"+state.Info().StoreId()+"::"+programModel.Id+"::"+programMeta.ProgramId+"::"+programMeta.Identifier, "true")
		uniqueMacs[fn.UserId] = append(uniqueMacs[fn.UserId], programMeta.Identifier)
	}
	for uniMacId, _ := range uniqueMacs {
		putAccessLinks(trx, uniMacId, state.Info().StoreId())
		a.App.Tools().Signaler().JoinGroup(state.Info().StoreId(), uniMacId)
	}
	trx.PutLink("storeMachine::"+state.Info().StoreId()+"::"+programModel.Id, "true")
	future.Async(func() {
		a.App.Tools().Signaler().SignalGroup("stores/addMachine", state.Info().StoreId(), updates_stores.AddMachine{StoreId: state.Info().StoreId(), Program: programModel, Programs: m}, true, []string{})
	}, false)
	return outputs_stores.AddMemberOutput{}, nil
}

// ListStoreMachines /stores/listMachines check [ true true false ] access [ true false false false POST ]
func (a *Actions) ListStoreMachines(state state.IState, input inputs_stores.ListStoreMachinesInput) (any, error) {
	trx := state.Trx()
	prefix := "storeMachineProgram::" + state.Info().StoreId() + "::"
	programLinks, err := trx.GetLinksList(prefix, 0, 1000)
	if err != nil {
		log.Println(err)
		return nil, err
	}
	fns := map[string]*updates_stores.Fn{}
	programsById := map[string]model.Machine{}
	for _, mlink := range programLinks {
		parts := strings.Split(mlink[len(prefix):], "::")
		machineId := parts[0]
		programId := parts[1]
		identifier := parts[2]
		program := model.User{Id: programId}.Pull(trx)
		metadata, err := trx.GetJson("FnMeta::"+state.Info().StoreId()+"::"+machineId+"::"+programId+"::"+identifier, "metadata")
		if err != nil {
			log.Println(err)
		}
		acc := map[string]bool{}
		rawAcc, err := trx.GetJson("StoreAccess::"+state.Info().StoreId()+"::"+programId, "metadata")
		if err != nil {
			acc = access
			log.Println(err)
		} else {
			for k, v := range rawAcc {
				acc[k] = v.(bool)
			}
		}
		meta, err := trx.GetJson("MachineMeta::"+programId, "metadata")
		if err != nil {
			log.Println(err)
			meta = map[string]any{}
		}
		maps.Copy(metadata, meta)
		programModel := model.Program{MachineId: programId}.Pull(trx)
		if err != nil {
			log.Println(err)
			return nil, err
		}
		fn := &updates_stores.Fn{
			UserId:     program.Id,
			Typ:        program.Typ,
			Username:   program.Username,
			PublicKey:  program.PublicKey,
			ProgramId:  programModel.AppId,
			Runtime:    programModel.Runtime,
			Path:       programModel.Path,
			Comment:    programModel.Comment,
			Metadata:   metadata,
			Identifier: identifier,
			Access:     acc,
		}

		if _, ok := programsById[fn.ProgramId]; !ok {
			programsById[fn.ProgramId] = model.Machine{Id: fn.ProgramId}.Pull(trx, true)
		}
		fns[fn.UserId+"::"+fn.Identifier] = fn
	}
	return outputs_stores.ListStoreMachinesOutput{Programs: fns, Models: programsById}, nil
}

// UpdateProgram /stores/updateProgram check [ true true false ] access [ true false false false POST ]
func (a *Actions) UpdateProgram(state state.IState, input inputs_stores.UpdateProgramInput) (any, error) {
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		if meta, err := state.Trx().GetJson("StoreAccess::"+state.Info().StoreId()+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["updateProgram"].(bool) {
			return nil, errors.New("access not permitted")
		}
	}
	trx := state.Trx()
	a.Locks.SetIfAbsent(state.Info().StoreId(), &LockHolder{})
	locker, _ := a.Locks.Get(state.Info().StoreId())
	locker.Lock.Lock()
	defer locker.Lock.Unlock()
	if !trx.HasObj("App", input.MachineId) {
		return nil, errors.New("machine not found")
	}
	programModel := model.Machine{Id: input.MachineId}.Pull(trx)
	trx.PutJson("FnMeta::"+state.Info().StoreId()+"::"+input.MachineId+"::"+input.ProgramMeta.ProgramId+"::"+input.ProgramMeta.Identifier, "metadata", input.ProgramMeta.Metadata, false)
	memberProgram := model.User{Id: input.ProgramMeta.ProgramId}.Pull(trx)
	program := model.Program{MachineId: input.ProgramMeta.ProgramId}.Pull(trx)
	fn := updates_stores.Fn{
		UserId:     memberProgram.Id,
		Typ:        memberProgram.Typ,
		Username:   memberProgram.Username,
		PublicKey:  memberProgram.PublicKey,
		ProgramId:  program.AppId,
		Runtime:    program.Runtime,
		Path:       program.Path,
		Comment:    program.Comment,
		Metadata:   input.ProgramMeta.Metadata,
		Identifier: input.ProgramMeta.Identifier,
	}
	future.Async(func() {
		a.App.Tools().Signaler().SignalGroup("stores/updateProgram", state.Info().StoreId(), updates_stores.UpdateMachine{StoreId: state.Info().StoreId(), Model: programModel, Program: fn}, true, []string{state.Info().UserId()})
	}, false)
	return outputs_stores.AddMemberOutput{}, nil
}

// RemoveMachine /stores/removeMachine check [ true true false ] access [ true false false false POST ]
func (a *Actions) RemoveMachine(state state.IState, input inputs_stores.RemoveMachineInput) (any, error) {
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		if meta, err := state.Trx().GetJson("StoreAccess::"+state.Info().StoreId()+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["removeMachine"].(bool) {
			return nil, errors.New("access not permitted")
		}
	}
	trx := state.Trx()
	a.Locks.SetIfAbsent(state.Info().StoreId(), &LockHolder{})
	locker, _ := a.Locks.Get(state.Info().StoreId())
	locker.Lock.Lock()
	defer locker.Lock.Unlock()
	if !trx.HasObj("App", input.MachineId) {
		return nil, errors.New("machine not found")
	}
	programModel := model.Machine{Id: input.MachineId}.Pull(trx)
	programs, err := model.User{}.List(trx, "machinePrograms::"+input.MachineId+"::", map[string]string{})
	if err != nil {
		log.Println(err)
		return nil, err
	}
	programModels, err := model.Program{}.List(trx, "machinePrograms::"+input.MachineId+"::")
	if err != nil {
		log.Println(err)
		return nil, err
	}
	programMap := map[string]model.Program{}
	for _, program := range programModels {
		programMap[program.MachineId] = program
	}
	macArr := strings.Split(trx.GetLink("storeMachinePrograms::"+state.Info().StoreId()+"::"+programModel.Id), ",")
	for _, program := range programs {
		if slices.Contains(macArr, program.Id) {
			delAccessLinks(trx, program.Id, state.Info().StoreId())
			trx.DelJson("StoreAccess::"+state.Info().StoreId()+"::"+program.Id, "metadata")
			a.App.Tools().Signaler().LeaveGroup(state.Info().StoreId(), program.Id)
		}
	}
	prefix := "storeMachineProgram::" + state.Info().StoreId() + "::" + programModel.Id + "::"
	arr, err := trx.GetLinksList(prefix, 0, 1000)
	if err != nil {
		log.Println(err)
	} else {
		for _, key := range arr {
			parts := strings.Split(key[len(prefix):], "::")
			trx.DelJson("FnMeta::"+state.Info().StoreId()+"::"+input.MachineId+"::"+parts[0]+"::"+parts[1], "metadata")
			trx.DelKey(key)
		}
	}
	trx.DelKey("link::storeMachine::" + state.Info().StoreId() + "::" + programModel.Id)
	future.Async(func() {
		a.App.Tools().Signaler().SignalGroup("stores/removeMachine", state.Info().StoreId(), updates_stores.AddMachine{StoreId: state.Info().StoreId(), Program: programModel}, true, []string{state.Info().UserId()})
	}, false)
	return outputs_stores.AddMemberOutput{}, nil
}

// AddProgram /stores/addProgram check [ true true false ] access [ true false false false POST ]
func (a *Actions) AddProgram(state state.IState, input inputs_stores.AddProgramInput) (any, error) {
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		if meta, err := state.Trx().GetJson("StoreAccess::"+state.Info().StoreId()+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["addProgram"].(bool) {
			return nil, errors.New("access not permitted")
		}
	}
	trx := state.Trx()
	a.Locks.SetIfAbsent(state.Info().StoreId(), &LockHolder{})
	locker, _ := a.Locks.Get(state.Info().StoreId())
	locker.Lock.Lock()
	defer locker.Lock.Unlock()
	if !trx.HasObj("App", input.MachineId) {
		return nil, errors.New("machine not found")
	}
	programModel := model.Machine{Id: input.MachineId}.Pull(trx)
	memberProgram := model.User{Id: input.ProgramMeta.ProgramId}.Pull(trx)
	program := model.Program{MachineId: input.ProgramMeta.ProgramId}.Pull(trx)
	meta, err := trx.GetJson("MachineMeta::"+program.MachineId, "metadata")
	if err != nil {
		log.Println(err)
		return nil, err
	}
	maps.Copy(input.ProgramMeta.Metadata, meta)
	acc := map[string]bool{}
	for k, v := range access {
		if v2, ok := input.ProgramMeta.Access[k]; ok {
			acc[k] = v2
		} else {
			acc[k] = v
		}
	}
	fn := updates_stores.Fn{
		UserId:     memberProgram.Id,
		Typ:        memberProgram.Typ,
		Username:   memberProgram.Username,
		PublicKey:  memberProgram.PublicKey,
		ProgramId:  program.AppId,
		Runtime:    program.Runtime,
		Path:       program.Path,
		Comment:    program.Comment,
		Identifier: input.ProgramMeta.Identifier,
		Metadata:   input.ProgramMeta.Metadata,
		Access:     acc,
	}
	if arr, err := trx.GetLinksList("storeMachineProgram::"+state.Info().StoreId()+"::"+fn.ProgramId+"::"+fn.UserId+"::", 0, 100); err != nil || len(arr) == 0 {
		trx.PutJson("StoreAccess::"+state.Info().StoreId()+"::"+fn.UserId, "metadata", acc, false)
	}
	trx.PutJson("FnMeta::"+state.Info().StoreId()+"::"+fn.ProgramId+"::"+fn.UserId+"::"+input.ProgramMeta.Identifier, "metadata", input.ProgramMeta.Metadata, true)
	putAccessLinks(trx, input.ProgramMeta.ProgramId, state.Info().StoreId())
	trx.PutLink("storeMachineProgram::"+state.Info().StoreId()+"::"+input.MachineId+"::"+input.ProgramMeta.ProgramId+"::"+input.ProgramMeta.Identifier, "true")
	a.App.Tools().Signaler().JoinGroup(state.Info().StoreId(), input.ProgramMeta.ProgramId)
	future.Async(func() {
		a.App.Tools().Signaler().SignalGroup("stores/addProgram", state.Info().StoreId(), updates_stores.AddProgram{StoreId: state.Info().StoreId(), Model: programModel, Program: fn}, true, []string{})
	}, false)
	return outputs_stores.AddMemberOutput{}, nil
}

// RemoveProgram /stores/removeProgram check [ true true false ] access [ true false false false POST ]
func (a *Actions) RemoveProgram(state state.IState, input inputs_stores.RemoveProgramInput) (any, error) {
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		if meta, err := state.Trx().GetJson("StoreAccess::"+state.Info().StoreId()+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["removeProgram"].(bool) {
			return nil, errors.New("access not permitted")
		}
	}
	trx := state.Trx()
	a.Locks.SetIfAbsent(state.Info().StoreId(), &LockHolder{})
	locker, _ := a.Locks.Get(state.Info().StoreId())
	locker.Lock.Lock()
	defer locker.Lock.Unlock()
	if !trx.HasObj("App", input.MachineId) {
		return nil, errors.New("machine not found")
	}
	if trx.GetLink("storeMachineProgram::"+state.Info().StoreId()+"::"+input.MachineId+"::"+input.ProgramId+"::"+input.Identifier) == "" {
		return nil, errors.New("program with this identifier does not exist in store")
	}
	programModel := model.Machine{Id: input.MachineId}.Pull(trx)
	memberProgram := model.User{Id: input.ProgramId}.Pull(trx)
	program := model.Program{MachineId: input.ProgramId}.Pull(trx)
	fn := updates_stores.Fn{
		UserId:     memberProgram.Id,
		Typ:        memberProgram.Typ,
		Username:   memberProgram.Username,
		PublicKey:  memberProgram.PublicKey,
		ProgramId:  program.AppId,
		Runtime:    program.Runtime,
		Path:       program.Path,
		Comment:    program.Comment,
		Identifier: input.Identifier,
	}
	trx.DelJson("FnMeta::"+state.Info().StoreId()+"::"+fn.ProgramId+"::"+fn.UserId+"::"+input.Identifier, "metadata")
	trx.DelKey("link::storeMachineProgram::" + state.Info().StoreId() + "::" + input.MachineId + "::" + input.ProgramId + "::" + input.Identifier)
	if arr, err := trx.GetLinksList("storeMachineProgram::"+state.Info().StoreId()+"::"+input.MachineId+"::"+input.ProgramId+"::", 0, 100); err == nil && len(arr) == 0 {
		delAccessLinks(trx, input.ProgramId, state.Info().StoreId())
		trx.DelJson("StoreAccess::"+state.Info().StoreId()+"::"+input.ProgramId, "metadata")
		a.App.Tools().Signaler().LeaveGroup(state.Info().StoreId(), input.ProgramId)
	}
	future.Async(func() {
		a.App.Tools().Signaler().SignalGroup("stores/removeProgram", state.Info().StoreId(), updates_stores.AddProgram{StoreId: state.Info().StoreId(), Model: programModel, Program: fn}, true, []string{})
	}, false)
	return outputs_stores.AddMemberOutput{}, nil
}

// AddMember /stores/addMember check [ true true false ] access [ true false false false POST ]
func (a *Actions) AddMember(state state.IState, input inputs_stores.AddMemberInput) (any, error) {
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		if meta, err := state.Trx().GetJson("StoreAccess::"+state.Info().StoreId()+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["addMember"].(bool) {
			return nil, errors.New("access not permitted")
		}
	}
	trx := state.Trx()
	a.Locks.SetIfAbsent(state.Info().StoreId(), &LockHolder{})
	locker, _ := a.Locks.Get(state.Info().StoreId())
	locker.Lock.Lock()
	defer locker.Lock.Unlock()
	if !trx.HasObj("User", input.UserId) {
		return nil, errors.New("user not found")
	}
	store := model.Store{Id: state.Info().StoreId()}.Pull(trx)
	if store.Tag == "home" {
		return nil, errors.New("home is not extendable")
	}
	if trx.GetLink("onaccess::"+state.Info().StoreId()+"::"+input.UserId) == "true" {
		return nil, errors.New("membership already exists")
	}
	putAccessLinks(trx, input.UserId, state.Info().StoreId())
	trx.PutJson("StoreAccess::"+state.Info().StoreId()+"::"+input.UserId, "metadata", access, false)
	acc := map[string]bool{}
	for k, v := range access {
		if v2, ok := input.Access[k]; ok {
			acc[k] = v2
		} else {
			acc[k] = v
		}
	}
	trx.PutJson("StoreAccess::"+state.Info().StoreId()+"::"+input.UserId, "metadata", acc, false)
	store.MemberCount++
	store.Push(trx)
	a.App.Tools().Signaler().JoinGroup(state.Info().StoreId(), input.UserId)
	user := model.User{Id: input.UserId}.Pull(trx)
	future.Async(func() {
		a.App.Tools().Signaler().SignalGroup("stores/addMember", state.Info().StoreId(), updates_stores.AddMember{StoreId: state.Info().StoreId(), User: user}, true, []string{state.Info().UserId()})
	}, false)
	return outputs_stores.AddMemberOutput{}, nil
}

// UpdateMember /stores/updateMember check [ true true true ] access [ true false false false POST ]
func (a *Actions) UpdateMember(state state.IState, input inputs_stores.UpdateMemberInput) (any, error) {
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		if meta, err := state.Trx().GetJson("StoreAccess::"+state.Info().StoreId()+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["updateMember"].(bool) {
			return nil, errors.New("access not permitted")
		}
	}
	trx := state.Trx()
	if state.Info().StoreId() == "" {
		return nil, errors.New("member not found")
	}
	store := model.Store{Id: state.Info().StoreId()}.Pull(trx)
	if store.Tag == "home" {
		return nil, errors.New("home is not extendable")
	}
	trx.PutJson("member_"+state.Info().StoreId()+"_"+input.UserId, "meta", input.Metadata, true)
	user := model.User{Id: input.UserId}.Pull(trx)
	obj, e := trx.GetJson("member_"+state.Info().StoreId()+"_"+input.UserId, "meta")
	if e == nil {
		future.Async(func() {
			a.App.Tools().Signaler().SignalGroup("stores/updateMember", state.Info().StoreId(), updates_stores.UpdateMember{StoreId: state.Info().StoreId(), User: user, Metadata: obj}, true, []string{state.Info().UserId()})
		}, false)
	}
	return outputs_stores.UpdateMemberOutput{Metadata: obj}, nil
}

// UpdateMemberAccess /stores/updateMemberAccess check [ true true true ] access [ true false false false POST ]
func (a *Actions) UpdateMemberAccess(state state.IState, input inputs_stores.UpdateMemberAccessInput) (any, error) {
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		return nil, errors.New("access not permitted")
	}
	trx := state.Trx()
	trx.PutJson("StoreAccess::"+state.Info().StoreId()+"::"+input.UserId, "metadata", input.Access, true)
	return map[string]any{}, nil
}

// UpdateProgramAccess /stores/updateProgramAccess check [ true true true ] access [ true false false false POST ]
func (a *Actions) UpdateProgramAccess(state state.IState, input inputs_stores.UpdateProgramAccessInput) (any, error) {
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		return nil, errors.New("access not permitted")
	}
	trx := state.Trx()
	trx.PutJson("StoreAccess::"+state.Info().StoreId()+"::"+input.ProgramId, "metadata", input.Access, true)
	return map[string]any{}, nil
}

// GetDefaultAccess /stores/getDefaultAccess check [ true false false ] access [ true false false false POST ]
func (a *Actions) GetDefaultAccess(state state.IState, input inputs_stores.GetDefaultAccessInput) (any, error) {
	return map[string]any{"access": access}, nil
}

func (a *Actions) getUserStatus(trx trx.ITrx, targetUserId string, requesterUserId string) string {
	if targetUserId == requesterUserId {
		if a.App.Tools().Signaler().Listeners().Has(targetUserId) {
			return "online"
		} else {
			return "offline"
		}
	} else {
		metaPriv, err := trx.GetJson("UserMeta::"+targetUserId, "metadata.private.settings.privacy")
		if err != nil {
			log.Println(err)
			return "last seen recently"
		}
		onlineView := metaPriv["onlineView"].(string)
		if onlineView == "all" {
			if a.App.Tools().Signaler().Listeners().Has(targetUserId) {
				return "online"
			} else {
				return "offline"
			}
		} else if onlineView == "contacts" {
			if metaCont, _ := trx.GetJson("UserMeta::"+targetUserId, "metadata.private.contacts"); metaCont[requesterUserId] == true {
				if a.App.Tools().Signaler().Listeners().Has(targetUserId) {
					return "online"
				} else {
					return "offline"
				}
			} else {
				return "last seen recently"
			}
		} else {
			return "last seen recently"
		}
	}
}

// ReadMembers /stores/readMembers check [ true true false ] access [ true false false false POST ]
func (a *Actions) ReadMembers(state state.IState, input inputs_stores.ReadMemberInput) (any, error) {
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		if meta, err := state.Trx().GetJson("StoreAccess::"+state.Info().StoreId()+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["readMembers"].(bool) {
			return nil, errors.New("access not permitted")
		}
	}
	trx := state.Trx()
	members, err := model.User{}.List(trx, "onaccess::"+state.Info().StoreId()+"::", map[string]string{"type": "human"})
	if err != nil {
		return nil, err
	}
	membersArr := []map[string]any{}
	isAdmin := trx.GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) == "true"
	for _, member := range members {
		if member.Typ == "human" {
			memberData := map[string]any{
				"id":        member.Id,
				"publicKey": member.PublicKey,
				"type":      member.Typ,
				"username":  member.Username,
			}
			for _, v := range a.modelExtender["user"] {
				if v.PrimaryProp {
					if meta, err := trx.GetJson("UserMeta::"+member.Id, v.Path); err == nil || meta[v.Name] != nil {
						memberData[v.Name] = meta[v.Name]
					}
				}
			}
			memberData["status"] = a.getUserStatus(trx, member.Id, state.Info().UserId())
			if isAdmin {
				acc := map[string]bool{}
				rawAcc, err := trx.GetJson("StoreAccess::"+state.Info().StoreId()+"::"+member.Id, "metadata")
				if err != nil {
					acc = access
					log.Println(err)
				} else {
					for k, v := range rawAcc {
						acc[k] = v.(bool)
					}
				}
				memberData["access"] = acc
			}
			membersArr = append(membersArr, memberData)
		}
	}
	return outputs_stores.ReadMemberOutput{Members: membersArr}, nil
}

// RemoveMember /stores/removeMember check [ true true false ] access [ true false false false POST ]
func (a *Actions) RemoveMember(state state.IState, input inputs_stores.RemoveMemberInput) (any, error) {
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		if meta, err := state.Trx().GetJson("StoreAccess::"+state.Info().StoreId()+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["removeMember"].(bool) {
			return nil, errors.New("access not permitted")
		}
	}
	trx := state.Trx()
	a.Locks.SetIfAbsent(state.Info().StoreId(), &LockHolder{})
	locker, _ := a.Locks.Get(state.Info().StoreId())
	locker.Lock.Lock()
	defer locker.Lock.Unlock()
	if trx.GetLink("onaccess::"+state.Info().StoreId()+"::"+input.UserId) != "true" {
		return nil, errors.New("member not found")
	}
	user := model.User{Id: input.UserId}.Pull(trx)
	if user.Typ != "human" {
		return nil, errors.New("member not found")
	}
	store := model.Store{Id: state.Info().StoreId()}.Pull(trx)
	if store.Tag == "home" {
		return nil, errors.New("home is not extendable")
	}
	if trx.GetLink("onaccess::"+state.Info().StoreId()+"::"+input.UserId) != "true" {
		return nil, errors.New("membership does exist")
	}
	delAccessLinks(trx, input.UserId, state.Info().StoreId())
	trx.DelJson("StoreAccess::"+state.Info().StoreId()+"::"+input.UserId, "metadata")
	store.MemberCount--
	store.Push(trx)
	a.App.Tools().Signaler().LeaveGroup(state.Info().StoreId(), input.UserId)
	future.Async(func() {
		a.App.Tools().Signaler().SignalGroup("stores/removeMember", state.Info().StoreId(), updates_stores.AddMember{StoreId: state.Info().StoreId(), User: user}, true, []string{state.Info().UserId()})
	}, false)
	return outputs_stores.AddMemberOutput{}, nil
}

// Create /stores/create check [ true false false ] access [ true false false false POST ]
func (a *Actions) Create(state state.IState, input inputs_stores.CreateInput) (any, error) {
	trx := state.Trx()
	orig := state.Source()
	if input.Origin() == "global" {
		orig = "global"
	}
	if input.Members == nil {
		input.Members = map[string]bool{}
	}
	input.Members[state.Info().UserId()] = true
	if input.Tag == "1-to-1" {
		if len(input.Members) > 2 {
			return nil, errors.New("1-to-1 chat can not have more than 2 members")
		} else if len(input.Members) == 2 && !input.Members[state.Info().UserId()] {
			return nil, errors.New("1-to-1 chat can not have more than 2 members")
		} else if len(input.Members) == 1 && input.Members[state.Info().UserId()] {
			return nil, errors.New("1-to-1 chat can not have more than 2 members")
		} else if len(input.Members) == 0 {
			return nil, errors.New("1-to-1 chat can not have more than 2 members")
		}
		for k := range input.Members {
			input.Members[k] = true
		}
		ids := []string{}
		for k := range input.Members {
			ids = append(ids, k)
		}
		slices.Sort(ids)
		a.OneToOneLocks.SetIfAbsent(ids[0]+"<->"+ids[1], &LockHolder{})
		locker, _ := a.OneToOneLocks.Get(ids[0] + "<->" + ids[1])
		locker.Lock.Lock()
		defer locker.Lock.Unlock()
		if storeId := trx.GetLink("1-to-1-map::" + ids[0] + "<->" + ids[1]); storeId != "" {
			store := model.Store{Id: storeId}.Pull(trx)
			return outputs_stores.CreateOutput{Store: outputs_stores.AdminPoiint{Store: store, Admin: true}}, nil
		}
	}
	if input.ParentId != "" {
		if !trx.HasObj("Store", input.ParentId) {
			err := errors.New("parent store does not exist")
			log.Println(err)
			return nil, err
		}
		if state.Trx().GetLink("admin::"+input.ParentId+"::"+state.Info().UserId()) != "true" {
			if meta, err := state.Trx().GetJson("StoreAccess::"+input.ParentId+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["createSubStore"].(bool) {
				return nil, errors.New("access not permitted")
			}
		}
	}
	store := model.Store{Id: a.App.Tools().Storage().GenId(trx, orig), MemberCount: int32(len(input.Members)), Tag: input.Tag, IsPublic: *input.IsPublic, PersHist: *input.PersHist, ParentId: input.ParentId}
	store.Push(trx)
	putAccessLinks(trx, state.Info().UserId(), store.Id)
	trx.PutLink("admin::"+store.Id+"::"+state.Info().UserId(), "true")
	trx.PutLink("adminof::"+state.Info().UserId()+"::"+store.Id, "true")
	if input.Members != nil {
		for userId, isAdmin := range input.Members {
			user := model.User{Id: userId}.Pull(trx)
			if user.Typ == "human" {
				putAccessLinks(trx, userId, store.Id)
				trx.PutJson("StoreAccess::"+store.Id+"::"+userId, "metadata", access, false)
				if isAdmin {
					trx.PutLink("admin::"+store.Id+"::"+userId, "true")
					trx.PutLink("adminof::"+userId+"::"+store.Id, "true")
				}
				a.App.Tools().Signaler().JoinGroup(store.Id, userId)
			} else {
				program := model.Program{MachineId: userId}.Pull(trx)
				meta, err := trx.GetJson("MachineMeta::"+program.MachineId, "metadata")
				if err != nil {
					log.Println(err)
					return nil, err
				}
				acc := map[string]bool{}
				maps.Copy(acc, access)
				if arr, err := trx.GetLinksList("storeMachineProgram::"+store.Id+"::"+program.AppId+"::"+program.MachineId+"::", 0, 100); err != nil || len(arr) == 0 {
					trx.PutJson("StoreAccess::"+store.Id+"::"+program.MachineId, "metadata", acc, false)
				}
				trx.PutJson("FnMeta::"+store.Id+"::"+program.AppId+"::"+program.MachineId+"::"+"0", "metadata", meta, true)
				putAccessLinks(trx, program.MachineId, store.Id)
				trx.PutLink("storeMachineProgram::"+store.Id+"::"+program.AppId+"::"+program.MachineId+"::"+"0", "true")
				if isAdmin {
					trx.PutLink("admin::"+store.Id+"::"+userId, "true")
					trx.PutLink("adminof::"+userId+"::"+store.Id, "true")
				}
				a.App.Tools().Signaler().JoinGroup(store.Id, program.MachineId)
			}
		}
	}
	for _, v := range a.modelExtender["store"] {
		trx.PutJson("StoreMeta::"+store.Id, v.Path, map[string]any{
			v.Name: v.Default,
		}, true)
	}
	if input.Metadata != nil {
		trx.PutJson("StoreMeta::"+store.Id, "metadata", input.Metadata, true)
	}
	for _, v := range a.modelExtender["store"] {
		if meta, err := trx.GetJson("StoreMeta::"+store.Id, v.Path); err != nil || meta[v.Name] == nil {
			if v.Required {
				return nil, errors.New(v.Name + " can not be empty.")
			}
		} else {
			if v.Searchable {
				trx.PutIndex("Store", v.Name, "id", store.Id+"->"+meta[v.Name].(string), []byte(store.Id))
			}
		}
	}
	if input.Tag == "1-to-1" {
		ids := []string{}
		for k := range input.Members {
			ids = append(ids, k)
		}
		slices.Sort(ids)
		trx.PutLink("1-to-1-map::"+ids[0]+"<->"+ids[1], store.Id)
	}
	store = store.Pull(trx)
	a.App.Tools().Signaler().SignalGroup("stores/create", store.Id, updates_stores.Delete{Store: store}, true, []string{state.Info().UserId()})
	return outputs_stores.CreateOutput{Store: outputs_stores.AdminPoiint{Store: store, Admin: true}}, nil
}

// Update /stores/update check [ true true false ] access [ true false false false PUT ]
func (a *Actions) Update(state state.IState, input inputs_stores.UpdateInput) (any, error) {
	trx := state.Trx()
	a.Locks.SetIfAbsent(state.Info().StoreId(), &LockHolder{})
	locker, _ := a.Locks.Get(state.Info().StoreId())
	locker.Lock.Lock()
	defer locker.Lock.Unlock()
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		if meta, err := state.Trx().GetJson("StoreAccess::"+state.Info().StoreId()+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["updateStore"].(bool) {
			return nil, errors.New("access not permitted")
		}
	}
	store := model.Store{Id: state.Info().StoreId()}.Pull(trx)
	if input.IsPublic != nil {
		store.IsPublic = *input.IsPublic
	}
	if input.PersHist != nil {
		store.PersHist = *input.PersHist
	}
	if input.Metadata != nil {
		for _, v := range a.modelExtender["store"] {
			if v.Searchable {
				if meta, err := trx.GetJson("StoreMeta::"+store.Id, v.Path); err == nil || meta[v.Name] != nil {
					trx.DelIndex("Store", v.Name, "id", store.Id+"->"+meta[v.Name].(string))
				}
			}
		}
		trx.PutJson("StoreMeta::"+store.Id, "metadata", input.Metadata, true)
		for _, v := range a.modelExtender["store"] {
			if meta, err := trx.GetJson("StoreMeta::"+store.Id, v.Path); err != nil || meta[v.Name] == nil {
				if v.Required {
					return nil, errors.New(v.Name + " can not be empty.")
				}
			} else {
				if v.Searchable {
					trx.PutIndex("Store", v.Name, "id", store.Id+"->"+meta[v.Name].(string), []byte(store.Id))
				}
			}
		}
	}
	store.Push(trx)
	future.Async(func() {
		a.App.Tools().Signaler().SignalGroup("stores/update", store.Id, updates_stores.Update{Store: store}, true, []string{state.Info().UserId()})
	}, false)
	return outputs_stores.UpdateOutput{Store: outputs_stores.AdminPoiint{Store: store, Admin: true}}, nil
}

// Delete /stores/delete check [ true true false ] access [ true false false false DELETE ]
func (a *Actions) Delete(state state.IState, input inputs_stores.DeleteInput) (any, error) {
	trx := state.Trx()
	a.Locks.SetIfAbsent(state.Info().StoreId(), &LockHolder{})
	locker, _ := a.Locks.Get(state.Info().StoreId())
	locker.Lock.Lock()
	defer locker.Lock.Unlock()
	if len(trx.GetColumn("Store", state.Info().StoreId(), "|")) == 0 {
		return nil, errors.New("store does not exist")
	}
	store := model.Store{Id: state.Info().StoreId()}.Pull(trx)
	if store.ParentId != "" {
		if trx.GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
			if meta, err := state.Trx().GetJson("StoreAccess::"+store.ParentId+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["deleteSubStore"].(bool) {
				return nil, errors.New("access not permitted")
			}
		}
	}
	if store.Tag == "home" {
		return nil, errors.New("your home can not be deleted")
	}
	for _, v := range a.modelExtender["store"] {
		if v.Searchable {
			if meta, err := trx.GetJson("StoreMeta::"+store.Id, v.Path); err == nil || meta[v.Name] != nil {
				trx.DelIndex("Store", v.Name, "id", store.Id+"->"+meta[v.Name].(string))
			}
		}
	}
	store.Delete(trx)
	members, _ := trx.GetLinksList("onaccess::"+store.Id+"::", 0, 0)
	usersList := []string{}
	for _, member := range members {
		parts := strings.Split(member, "::")
		delAccessLinks(trx, parts[2], parts[1])
		trx.DelKey("link::" + member)
		trx.DelJson("StoreAccess::"+parts[2]+"::"+parts[1], "metadata")
		usersList = append(usersList, parts[1])
	}
	prefix := "admin::" + store.Id + "::"
	admins, _ := trx.GetLinksList(prefix, 0, 0)
	for _, admin := range admins {
		trx.DelKey("link::" + admin)
		trx.DelKey("link::adminof::" + admin[len(prefix):] + "::" + store.Id)
	}
	future.Async(func() {
		a.App.Tools().Signaler().SignalGroup("stores/delete", store.Id, updates_stores.Delete{Store: store}, true, []string{state.Info().UserId()})
		for _, user := range usersList {
			a.App.Tools().Signaler().LeaveGroup(store.Id, user)
		}
	}, false)
	a.Locks.Remove(state.Info().StoreId())
	return outputs_stores.DeleteOutput{Store: outputs_stores.AdminPoiint{Store: store, Admin: true}}, nil
}

// Meta /stores/meta check [ true false false ] access [ true false false false GET ]
func (a *Actions) Meta(state state.IState, input inputs_stores.MetaInput) (any, error) {
	trx := state.Trx()
	if !trx.HasObj("Store", input.StoreId) {
		return nil, errors.New("store not found")
	}
	isMember := a.App.Tools().Security().HasAccessToStore(state.Info().StoreId(), input.StoreId)
	if !isMember && strings.HasPrefix(input.Path, "private.") {
		return nil, errors.New("access denied")
	}
	metadata, err := trx.GetJson("StoreMeta::"+input.StoreId, "metadata."+input.Path)
	if err != nil {
		log.Println(err)
		return nil, err
	}
	return map[string]any{"metadata": metadata}, nil
}

// Get /stores/get check [ true false false ] access [ true false false false GET ]
func (a *Actions) Get(state state.IState, input inputs_stores.GetInput) (any, error) {
	trx := state.Trx()
	if !trx.HasObj("Store", input.StoreId) {
		return nil, errors.New("store not found")
	}
	store := model.Store{Id: input.StoreId}.Pull(trx)
	if store.IsPublic {
		result := map[string]any{
			"id":          store.Id,
			"parentId":    store.ParentId,
			"isPublic":    store.IsPublic,
			"persHist":    store.PersHist,
			"memberCount": store.MemberCount,
			"signalCount": store.SignalCount,
			"tag":         store.Tag,
		}
		for _, v := range a.modelExtender["store"] {
			if meta, err := trx.GetJson("StoreMeta::"+store.Id, v.Path); err == nil || meta[v.Name] != nil {
				result[v.Name] = meta[v.Name]
			}
		}
		if input.IncludeMeta {
			metadata, err := trx.GetJson("StoreMeta::"+store.Id, "metadata")
			if err != nil {
				log.Println(err)
				return nil, err
			}
			result["metadata"] = metadata
		}
		if trx.GetLink("adminof::"+state.Info().UserId()+"::"+store.Id) == "true" {
			result["admin"] = true
		}
		return outputs_stores.GetOutput{Store: result}, nil
	}
	if trx.GetLink("onaccess::"+input.StoreId+"::"+state.Info().UserId()) != "true" {
		return nil, errors.New("access to private store denied")
	}
	lastPacket := map[string]any{}
	lpData, err := trx.GetJson("StoreMeta::"+store.Id, "metadata.public.lastPacket")
	if err == nil {
		lastPacket = lpData
	}
	result := map[string]any{
		"id":          store.Id,
		"parentId":    store.ParentId,
		"isPublic":    store.IsPublic,
		"persHist":    store.PersHist,
		"memberCount": store.MemberCount,
		"signalCount": store.SignalCount,
		"tag":         store.Tag,
		"lastPacket":  lastPacket,
	}
	for _, v := range a.modelExtender["store"] {
		if meta, err := trx.GetJson("StoreMeta::"+store.Id, v.Path); err == nil || meta[v.Name] != nil {
			result[v.Name] = meta[v.Name]
		}
	}
	if input.IncludeMeta {
		metadata, err := trx.GetJson("StoreMeta::"+store.Id, "metadata")
		if err != nil {
			log.Println(err)
			return nil, err
		}
		result["metadata"] = metadata
	}
	if trx.GetLink("adminof::"+state.Info().UserId()+"::"+store.Id) == "true" {
		result["admin"] = true
	}
	return outputs_stores.GetOutput{Store: result}, nil
}

// Read /stores/read check [ true false false ] access [ true false false false GET ]
func (a *Actions) Read(state state.IState, input inputs_stores.ReadInput) (any, error) {
	trx := state.Trx()
	stores, err := model.Store{}.List(trx, "hasaccess::"+state.Info().UserId()+"::", input.Orig == "global", map[string]string{},
		map[string][]string{
			"tag": {"group", "1-to-1", "home"},
		}, input.Offset, input.Count)
	if err != nil {
		log.Println(err)
		return nil, err
	}
	results := []map[string]any{}
	for _, store := range stores {
		lastPacket := map[string]any{}
		lpData, err := trx.GetJson("StoreMeta::"+store.Id, "metadata.public.lastPacket")
		if err == nil {
			lastPacket = lpData
		}
		result := map[string]any{
			"id":          store.Id,
			"parentId":    store.ParentId,
			"isPublic":    store.IsPublic,
			"persHist":    store.PersHist,
			"memberCount": store.MemberCount,
			"signalCount": store.SignalCount,
			"tag":         store.Tag,
			"lastPacket":  lastPacket,
		}
		for _, v := range a.modelExtender["store"] {
			if meta, err := trx.GetJson("StoreMeta::"+store.Id, v.Path); err == nil || meta[v.Name] != nil {
				result[v.Name] = meta[v.Name]
			}
		}
		if trx.GetLink("adminof::"+state.Info().UserId()+"::"+store.Id) == "true" {
			result["admin"] = true
		}
		results = append(results, result)
	}
	return outputs_stores.ReadOutput{Stores: results}, nil
}

// Join /stores/join check [ true false false ] access [ true false false false POST ]
func (a *Actions) Join(state state.IState, input inputs_stores.JoinInput) (any, error) {
	trx := state.Trx()
	a.Locks.SetIfAbsent(state.Info().StoreId(), &LockHolder{})
	locker, _ := a.Locks.Get(state.Info().StoreId())
	locker.Lock.Lock()
	defer locker.Lock.Unlock()
	if !trx.HasObj("Store", input.StoreId) {
		return nil, errors.New("store not found")
	}
	store := model.Store{Id: input.StoreId}.Pull(trx)
	if !store.IsPublic {
		return nil, errors.New("store is private")
	}
	if trx.GetLink("onaccess::"+store.Id+"::"+state.Info().UserId()) == "true" {
		return nil, errors.New("membership already eixsts")
	}
	putAccessLinks(trx, state.Info().UserId(), store.Id)
	trx.PutJson("StoreAccess::"+store.Id+"::"+state.Info().UserId(), "metadata", access, false)
	store.MemberCount++
	store.Push(trx)
	a.App.Tools().Signaler().JoinGroup(store.Id, state.Info().UserId())
	user := model.User{Id: state.Info().UserId()}.Pull(trx)
	future.Async(func() {
		a.App.Tools().Signaler().SignalGroup("stores/join", store.Id, updates_stores.Join{StoreId: store.Id, User: user}, true, []string{state.Info().UserId()})
	}, false)
	return outputs_stores.JoinOutput{}, nil
}

// Leave /stores/leave check [ true true false ] access [ true false false false POST ]
func (a *Actions) Leave(state state.IState, input inputs_stores.JoinInput) (any, error) {
	trx := state.Trx()
	a.Locks.SetIfAbsent(state.Info().StoreId(), &LockHolder{})
	locker, _ := a.Locks.Get(state.Info().StoreId())
	locker.Lock.Lock()
	defer locker.Lock.Unlock()
	if !trx.HasObj("Store", input.StoreId) {
		return nil, errors.New("store not found")
	}
	store := model.Store{Id: input.StoreId}.Pull(trx)
	if !store.IsPublic {
		return nil, errors.New("store is private")
	}
	if trx.GetLink("onaccess::"+store.Id+"::"+state.Info().UserId()) != "true" {
		return nil, errors.New("membership doesn't eixst")
	}
	delAccessLinks(trx, state.Info().UserId(), store.Id)
	trx.DelJson("StoreAccess::"+store.Id+"::"+state.Info().UserId(), "metadata")
	if trx.GetLink("admin::"+store.Id+"::"+state.Info().UserId()) == "true" {
		trx.DelKey("link::admin::" + store.Id + "::" + state.Info().UserId())
		trx.DelKey("lnik::adminof::" + state.Info().UserId() + "::" + store.Id)
	}
	store.MemberCount--
	store.Push(trx)
	a.App.Tools().Signaler().LeaveGroup(store.Id, state.Info().UserId())
	user := model.User{Id: state.Info().UserId()}.Pull(trx)
	future.Async(func() {
		a.App.Tools().Signaler().SignalGroup("stores/join", store.Id, updates_stores.Join{StoreId: store.Id, User: user}, true, []string{state.Info().UserId()})
	}, false)
	if store.MemberCount == 0 {
		for _, v := range a.modelExtender["store"] {
			if v.Searchable {
				if meta, err := trx.GetJson("StoreMeta::"+store.Id, v.Path); err == nil || meta[v.Name] != nil {
					trx.DelIndex("Store", v.Name, "id", store.Id+"->"+meta[v.Name].(string))
				}
			}
		}
		store.Delete(trx)
		a.Locks.Remove(state.Info().StoreId())
	}
	return outputs_stores.JoinOutput{}, nil
}

// Signal /stores/signal check [ true true true ] access [ true false false false POST ]
func (a *Actions) Signal(state state.IState, input inputs_stores.SignalInput) (any, error) {
	if matched, res, err := a.handleGodSignalCommand(state, input); matched {
		return res, err
	}
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		if meta, err := state.Trx().GetJson("StoreAccess::"+state.Info().StoreId()+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["sendSignal"].(bool) {
			return nil, errors.New("access not permitted")
		}
	}
	trx := state.Trx()
	a.Locks.SetIfAbsent(state.Info().StoreId(), &LockHolder{})
	locker, _ := a.Locks.Get(state.Info().StoreId())
	locker.Lock.Lock()
	defer locker.Lock.Unlock()
	store := model.Store{Id: state.Info().StoreId()}.Pull(trx)
	user := model.User{Id: state.Info().UserId()}.Pull(trx)
	t := time.Now().UnixMilli()
	if input.Type == "broadcast" {
		if store.PersHist && !input.Temp {
			packet := a.App.Tools().Storage().LogTimeSieries(store.Id, user.Id, input.Data, t)
			trx.PutJson("StoreMeta::"+store.Id, "metadata.public.lastPacket", packet, false)
			store.SignalCount++
			store.Push(trx)
			var p = updates_stores.Send{Id: packet.Id, Action: "broadcast", Store: store, User: user, Data: input.Data, Time: t}
			future.Async(func() {
				a.App.Tools().Signaler().SignalGroup("creatures/signal", store.Id, p, true, []string{state.Info().UserId()})
			}, false)
			return outputs_stores.SignalOutput{Passed: true, Packet: packet}, nil
		} else {
			var p = updates_stores.Send{Action: "broadcast", Store: store, User: user, Data: input.Data, Time: t, IsTemp: true}
			future.Async(func() {
				a.App.Tools().Signaler().SignalGroup("creatures/signal", store.Id, p, true, []string{state.Info().UserId()})
			}, false)
			return outputs_stores.SignalOutput{Passed: true}, nil
		}
	} else if input.Type == "single" {
		if trx.GetLink("onaccess::"+store.Id+"::"+input.UserId) == "true" {
			if store.PersHist && !input.Temp {
				packet := a.App.Tools().Storage().LogTimeSieries(store.Id, user.Id, input.Data, t)
				trx.PutJson("StoreMeta::"+store.Id, "metadata.public.lastPacket", packet, false)
				store.SignalCount++
				store.Push(trx)
				var p = updates_stores.Send{Id: packet.Id, Action: "single", Store: store, User: user, Data: input.Data, Time: t}
				future.Async(func() {
					a.App.Tools().Signaler().SignalUser("creatures/signal", input.UserId, p, true)
				}, false)
				return outputs_stores.SignalOutput{Passed: true, Packet: packet}, nil
			} else {
				var p = updates_stores.Send{Action: "single", Store: store, User: user, Data: input.Data, Time: t, IsTemp: true}
				future.Async(func() {
					a.App.Tools().Signaler().SignalUser("creatures/signal", input.UserId, p, true)
				}, false)
				return outputs_stores.SignalOutput{Passed: true}, nil
			}
		}
	}
	return outputs_stores.SignalOutput{Passed: false}, nil
}

// History /stores/history check [ true true true ] access [ true false false false POST ]
func (a *Actions) History(state state.IState, input inputs_stores.HistoryInput) (any, error) {
	if state.Trx().GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		if meta, err := state.Trx().GetJson("StoreAccess::"+state.Info().StoreId()+"::"+state.Info().UserId(), "metadata"); err != nil || !meta["readHistory"].(bool) {
			return nil, errors.New("access not permitted")
		}
	}
	if len(input.Ids) == 0 {
		return outputs_stores.HistoryOutput{Packets: a.App.Tools().Storage().ReadStoreLogs(state.Info().StoreId(), input.BeforeTime, input.Count)}, nil
	} else {
		return outputs_stores.HistoryOutput{Packets: a.App.Tools().Storage().PickStoreLogs(state.Info().StoreId(), input.Ids)}, nil
	}
}

// List /stores/list check [ true false false ] access [ true false false false GET ]
func (a *Actions) List(state state.IState, input inputs_stores.ListInput) (any, error) {
	trx := state.Trx()
	stores, err := model.Store{}.Search(trx, input.Offset, input.Count, input.Query, map[string]string{"isPublic": string([]byte{0x01}), "tag": "group"})
	if err != nil {
		log.Println(err)
		return nil, err
	}
	results := []map[string]any{}
	for _, store := range stores {
		result := map[string]any{
			"id":          store.Id,
			"parentId":    store.ParentId,
			"isPublic":    store.IsPublic,
			"persHist":    store.PersHist,
			"memberCount": store.MemberCount,
			"tag":         store.Tag,
		}
		for _, v := range a.modelExtender["store"] {
			if meta, err := trx.GetJson("StoreMeta::"+store.Id, v.Path); err == nil || meta[v.Name] != nil {
				result[v.Name] = meta[v.Name]
			}
		}
		if trx.GetLink("adminof::"+state.Info().UserId()+"::"+store.Id) == "true" {
			result["admin"] = true
		}
		results = append(results, result)
	}
	return map[string]any{"stores": results}, nil
}
