package actions_machine

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"kasper/src/abstract/models/core"
	"kasper/src/abstract/models/trx"
	"kasper/src/abstract/state"
	inputs_machiner "kasper/src/shell/api/inputs/machine"
	"kasper/src/shell/api/model"
	outputs_machiner "kasper/src/shell/api/outputs/plugin"
	updates_points "kasper/src/shell/api/updates/points"
	"kasper/src/shell/utils/future"
	"log"
	"strconv"
	"strings"
	"time"

	"github.com/google/uuid"
)

const pluginsTemplateName = "/machines/"

type Actions struct {
	App core.ICore
}

func Install(a *Actions, extra ...any) error {
	a.App.ModifyState(true, func(trx trx.ITrx) error {
		programs, err := model.Program{}.All(trx, -1, -1)
		if err != nil {
			panic(err)
		}
		for _, program := range programs {
			if program.Runtime == "wasm" || program.Runtime == "elpify" || program.Runtime == "javascript" {
				a.App.Tools().Vmm().Assign(program.MachineId)
				if pointId := trx.GetLink("vmAlarmPointId::" + program.MachineId); pointId != "" {
					future.Async(func() {
						t, _ := strconv.ParseInt(trx.GetLink("vmAlarmTime::"+program.MachineId), 10, 64)
						ct := time.Now().UnixMilli()
						if t > ct {
							time.Sleep(time.Duration(t-ct) * time.Millisecond)
						}
						data := trx.GetLink("vmAlarmData::" + program.MachineId)
						trx.DelKey("link::vmAlarmPointId::" + program.MachineId)
						trx.DelKey("link::vmAlarmData::" + program.MachineId)
						trx.DelKey("link::vmAlarmTime::" + program.MachineId)
						if a.App.Tools().Security().HasAccessToPoint(program.MachineId, pointId) {
							a.App.Tools().Vmm().RunVm(program.MachineId, pointId, data)
						}
					}, false)
				}
			} else if program.Runtime == "elpis" {
				a.App.Tools().Elpis().Assign(program.MachineId)
			} else if program.Runtime == "docker" {
				a.App.Tools().Docker().Assign(program.MachineId)
				if trx.GetLink("vmStatus::"+program.MachineId) == "running" {
					future.Async(func() {
						a.App.Tools().Docker().SaRContainer(program.MachineId, "main", "main")
						a.App.Tools().Docker().RunContainer(program.MachineId, "", "main", "main", map[string]string{}, true)
					}, false)
				}
			}
			var pointIds []string
			prefix := "memberof::" + program.MachineId + "::"
			pIds, err := trx.GetLinksList(prefix, -1, -1)
			if err != nil {
				log.Println(err)
				pointIds = []string{}
			} else {
				pointIds = pIds
			}
			for _, pointId := range pointIds {
				a.App.Tools().Signaler().JoinGroup(pointId[len(prefix):], program.MachineId)
			}
		}
		return nil
	})
	return nil
}

// CreateProgram /programs/create check [ true false false ] access [ true false false false POST ]
func (a *Actions) CreateProgram(state state.IState, input inputs_machiner.CreateAppInput) (any, error) {
	trx := state.Trx()
	if trx.HasIndex("App", "username", "id", input.Username) {
		return nil, errors.New("app username already exists")
	}
	shardChainId := "shard-main"
	if input.ShardChainId != nil && *input.ShardChainId != "" {
		shardChainId = *input.ShardChainId
	}
	machine := model.Machine{Id: a.App.Tools().Storage().GenId(trx, input.Origin()), MachinesCount: 0, Username: input.Username, OwnerId: state.Info().UserId(), ChainId: input.ChainId, ShardChainId: shardChainId}
	machine.Push(trx)
	trx.PutJson("AppMeta::"+machine.Id, "metadata", input.Metadata, false)
	profile, err := trx.GetJson("AppMeta::"+machine.Id, "metadata.public.profile")
	if err != nil {
		log.Println(err)
		return nil, err
	}
	if profile["title"] == nil {
		return nil, errors.New("title can not be empty")
	}
	if profile["desc"] == nil {
		return nil, errors.New("description can not be empty")
	}
	if profile["avatar"] == nil {
		return nil, errors.New("avatar can not be empty")
	}
	trx.PutLink("createdApp::"+state.Info().UserId()+"::"+machine.Id, "true")
	trx.PutIndex("App", "title", "id", machine.Id+"->"+profile["title"].(string), []byte(machine.Id))
	a.App.Tools().Network().Chain().NotifyNewMachineCreated(input.ChainId, machine.Id)
	return map[string]any{"app": machine}, nil
}

// DeleteProgram /programs/delete check [ true false false ] access [ true false false false POST ]
func (a *Actions) DeleteProgram(state state.IState, input inputs_machiner.DeleteAppInput) (any, error) {
	trx := state.Trx()
	if !trx.HasObj("App", input.AppId) {
		return nil, errors.New("app does not exist")
	}
	profile, err := trx.GetJson("AppMeta::"+input.AppId, "metadata.public.profile")
	if err == nil {
		trx.DelIndex("App", "title", "id", input.AppId+"->"+profile["title"].(string))
	} else {
		log.Println(err)
	}
	model.Machine{Id: input.AppId}.Delete(trx)
	trx.DelKey("link::createdApp::" + state.Info().UserId() + "::" + input.AppId)
	return map[string]any{}, nil
}

// UpdateProgram /programs/update check [ true false false ] access [ true false false false POST ]
func (a *Actions) UpdateProgram(state state.IState, input inputs_machiner.UpdateAppInput) (any, error) {
	trx := state.Trx()
	if !trx.HasObj("App", input.AppId) {
		return nil, errors.New("machine does not exist")
	}
	profile, err := trx.GetJson("AppMeta::"+input.AppId, "metadata.public.profile")
	if err == nil {
		trx.DelIndex("App", "title", "id", input.AppId+"->"+profile["title"].(string))
	} else {
		log.Println(err)
	}
	trx.PutJson("AppMeta::"+input.AppId, "metadata", input.Metadata, true)
	profile, err = trx.GetJson("AppMeta::"+input.AppId, "metadata.public.profile")
	if err != nil {
		log.Println(err)
		return nil, err
	}
	if profile["title"] == nil {
		return nil, errors.New("title can not be empty")
	}
	if profile["desc"] == nil {
		return nil, errors.New("description can not be empty")
	}
	if profile["avatar"] == nil {
		return nil, errors.New("avatar can not be empty")
	}
	trx.PutIndex("App", "title", "id", input.AppId+"->"+profile["title"].(string), []byte(input.AppId))
	return map[string]any{}, nil
}

// MyCreatedPrograms /programs/myCreated check [ true false false ] access [ true false false false GET ]
func (a *Actions) MyCreatedPrograms(state state.IState, input inputs_machiner.ListInput) (any, error) {
	trx := state.Trx()
	machines, err := model.Machine{}.List(trx, "createdApp::"+state.Info().UserId()+"::")
	if err != nil {
		log.Println(err)
		return nil, err
	}
	result := []map[string]any{}
	for _, machine := range machines {
		profile, err := trx.GetJson("AppMeta::"+machine.Id, "metadata.public.profile")
		if err != nil {
			log.Println(err)
			result = append(result, map[string]any{
				"id":            machine.Id,
				"chainId":       machine.ChainId,
				"shardChainId":  machine.ShardChainId,
				"username":      machine.Username,
				"ownerId":       machine.OwnerId,
				"machinesCount": machine.MachinesCount,
				"title":         "untitled",
				"avatar":        "",
				"desc":          "",
			})
			continue
		}
		result = append(result, map[string]any{
			"id":            machine.Id,
			"chainId":       machine.ChainId,
			"shardChainId":  machine.ShardChainId,
			"username":      machine.Username,
			"ownerId":       machine.OwnerId,
			"machinesCount": machine.MachinesCount,
			"title":         profile["title"],
			"avatar":        profile["avatar"],
			"desc":          profile["desc"],
		})
	}
	return map[string]any{"apps": result}, nil
}

// CreateMachine /machines/create check [ true false false ] access [ true false false false POST ]
func (a *Actions) CreateMachine(state state.IState, input inputs_machiner.CreateMachineInput) (any, error) {
	var (
		user    model.User
		session model.Session
	)
	trx := state.Trx()
	if !trx.HasObj("App", input.AppId) {
		return nil, errors.New("app not found")
	}
	machine := model.Machine{Id: input.AppId}.Pull(trx)
	if machine.OwnerId != state.Info().UserId() {
		return nil, errors.New("you are not owner of app")
	}
	user = model.User{Id: a.App.Tools().Storage().GenId(trx, input.Origin()), Balance: 1000, Typ: "machine", PublicKey: input.PublicKey, Username: input.Username + "@" + state.Source()}
	session = model.Session{Id: a.App.Tools().Storage().GenId(trx, input.Origin()), UserId: user.Id}
	program := model.Program{MachineId: user.Id, AppId: machine.Id, Path: input.Path, Runtime: input.Runtime, Comment: input.Comment}
	machine.MachinesCount++
	machine.Push(trx)
	user.Push(trx)
	session.Push(trx)
	program.Push(trx)
	trx.PutJson("MachineMeta::"+program.MachineId, "metadata", map[string]any{}, true)
	trx.PutIndex("Machine", "id", "appId", user.Id, []byte(machine.Id))
	trx.PutLink("appMachines::"+machine.Id+"::"+program.MachineId, "true")
	return outputs_machiner.CreateOutput{User: user}, nil
}

// DeleteMachine /machines/delete check [ true false false ] access [ true false false false POST ]
func (a *Actions) DeleteMachine(state state.IState, input inputs_machiner.DeleteMachineInput) (any, error) {
	trx := state.Trx()
	if !trx.HasObj("User", input.MachineId) {
		return nil, errors.New("machine does not exist")
	}
	model.User{Id: input.MachineId}.Delete(trx)
	appId := trx.GetIndex("Machine", "id", "appId", input.MachineId)
	machine := model.Machine{Id: appId}.Pull(trx)
	machine.MachinesCount--
	machine.Push(trx)
	trx.DelIndex("Machine", "id", "appId", input.MachineId)
	trx.DelKey("link::appMachines::" + machine.Id + "::" + input.MachineId)
	return map[string]any{}, nil
}

// UpdateMachine /machines/update check [ true false false ] access [ true false false false POST ]
func (a *Actions) UpdateMachine(state state.IState, input inputs_machiner.UpdateMachineInput) (any, error) {
	trx := state.Trx()
	if !trx.HasObj("User", input.MachineId) {
		return nil, errors.New("machine does not exist")
	}
	program := model.Program{MachineId: input.MachineId}.Pull(trx)
	program.Path = input.Path
	program.Push(trx)
	if input.Metadata != nil {
		trx.PutJson("MachineMeta::"+program.MachineId, "metadata", input.Metadata, true)
	}
	return map[string]any{}, nil
}

// Signal /machines/signal check [ true false false ] access [ true false false false POST ]
func (a *Actions) Signal(state state.IState, input inputs_machiner.SignalInput) (any, error) {
	trx := state.Trx()
	user := model.User{Id: state.Info().UserId()}.Pull(trx)
	program := model.Program{MachineId: input.MachineId}.Pull(trx)
	var p = updates_points.Send{Action: "single", User: user, Data: input.Data}
	future.Async(func() {
		a.App.Tools().Signaler().SignalUser("points/signal", program.MachineId+"_"+input.VmTag, p, true)
	}, false)
	return map[string]any{}, nil
}

// RunProgramEntity /machines/runProgramEntity check [ true false false ] access [ true false false false POST ]
func (a *Actions) RunProgramEntity(state state.IState, input inputs_machiner.RunMachineInput) (any, error) {
	trx := state.Trx()
	if !trx.HasObj("User", input.MachineId) {
		return nil, errors.New("machine does not exist")
	}
	program := model.Program{MachineId: input.MachineId}.Pull(trx)
	entity := model.Entity{ProgramId: program.AppId, EntityId: input.EntityId}.Pull(trx)
	if entity.EntityId == "" {
		return nil, errors.New("entity does not exist")
	}
	entityType := strings.ToLower(strings.TrimSpace(entity.EntityType))
	machine := model.Machine{Id: program.AppId}.Pull(trx)
	if machine.OwnerId != state.Info().UserId() {
		return nil, errors.New("you are not owner of this machine")
	}
	trx.PutLink("machineStatus::"+program.MachineId, "running")
	params := input.Params
	if params == nil {
		params = map[string]string{}
	}
	vmId := uuid.NewString()
	trx.PutLink("vmStandaloneRunId::"+program.MachineId+"::"+input.EntityId, vmId)
	if entityType == "docker" {
		imageName := uuid.NewString()
		containerName := uuid.NewString()
		trx.PutLink("vmStandaloneImageName::"+program.MachineId+"::"+input.EntityId, imageName)
		trx.PutLink("vmStandaloneContainerName::"+program.MachineId+"::"+input.EntityId, containerName)
		future.Async(func() {
			msg, _ := json.Marshal(map[string]any{
				"key": "runVm",
				"input": map[string]any{
					"runtime":       entityType,
					"machineId":     input.MachineId,
					"entityId":      input.EntityId,
					"imageName":     imageName,
					"containerName": containerName,
					"standalone":    true,
					"vmId":          vmId,
					"imageRef":      strings.ReplaceAll(input.MachineId, "@", "_") + "/" + entity.ImageName,
					"inputFiles":    params,
				},
			})
			a.App.Tools().Vmm().VmCallback(string(msg))
		}, false)
		return map[string]any{"vmId": vmId}, nil
	}
	if entityType != "wasm" && entityType != "javascript" && entityType != "elpify" {
		return nil, errors.New("invalid entity type")
	}
	data, _ := json.Marshal(params)
	future.Async(func() {
		msg, _ := json.Marshal(map[string]any{
			"key": "runVm",
			"input": map[string]any{
				"runtime":    entityType,
				"machineId":  input.MachineId,
				"entityId":   input.EntityId,
				"standalone": true,
				"vmId":       vmId,
				"data":       string(data),
			},
		})
		a.App.Tools().Vmm().VmCallback(string(msg))
	}, false)
	return map[string]any{"vmId": vmId}, nil
}

// StopProgramEntity /machines/stopProgramEntity check [ true false false ] access [ true false false false POST ]
func (a *Actions) StopProgramEntity(state state.IState, input inputs_machiner.RunMachineInput) (any, error) {
	trx := state.Trx()
	if !trx.HasObj("User", input.MachineId) {
		return nil, errors.New("machine does not exist")
	}
	program := model.Program{MachineId: input.MachineId}.Pull(trx)
	entity := model.Entity{ProgramId: program.AppId, EntityId: input.EntityId}.Pull(trx)
	if entity.EntityId == "" {
		return nil, errors.New("entity does not exist")
	}
	entityType := strings.ToLower(strings.TrimSpace(entity.EntityType))
	machine := model.Machine{Id: program.AppId}.Pull(trx)
	if machine.OwnerId != state.Info().UserId() {
		return nil, errors.New("you are not owner of this machine")
	}
	vmId := trx.GetLink("vmStandaloneRunId::" + program.MachineId + "::" + input.EntityId)
	if vmId == "" {
		return nil, errors.New("entity is not running")
	}
	trx.DelKey("link::machineStatus::" + program.MachineId)
	trx.DelKey("link::vmStandaloneRunId::" + program.MachineId + "::" + input.EntityId)
	imageName := trx.GetLink("vmStandaloneImageName::" + program.MachineId + "::" + input.EntityId)
	containerName := trx.GetLink("vmStandaloneContainerName::" + program.MachineId + "::" + input.EntityId)
	trx.DelKey("link::vmStandaloneImageName::" + program.MachineId + "::" + input.EntityId)
	trx.DelKey("link::vmStandaloneContainerName::" + program.MachineId + "::" + input.EntityId)
	stopInput := map[string]any{
		"runtime":    entityType,
		"machineId":  input.MachineId,
		"entityId":   input.EntityId,
		"standalone": true,
		"vmId":       vmId,
	}
	if entityType == "docker" {
		if imageName == "" || containerName == "" {
			return nil, errors.New("entity runtime links are not found")
		}
		stopInput["imageName"] = imageName
		stopInput["containerName"] = containerName
	}
	msg, _ := json.Marshal(map[string]any{
		"key":   "terminateVm",
		"input": stopInput,
	})
	a.App.Tools().Vmm().VmCallback(string(msg))
	return map[string]any{}, nil
}

// ReadBuildLogs /machines/readBuildLogs check [ true false false ] access [ true false false false POST ]
func (a *Actions) ReadBuildLogs(state state.IState, input inputs_machiner.ReadBuildLogsInput) (any, error) {
	if state.Trx().GetLink("vmBuilds::"+input.MachineId+"::"+input.BuildId) != "true" {
		return nil, errors.New("build not found")
	}
	return map[string]any{"logs": a.App.Tools().Storage().ReadBuildLogs(input.BuildId, input.MachineId)}, nil
}

// ReadMachineBuilds /machines/readMachineBuilds check [ true false false ] access [ true false false false POST ]
func (a *Actions) ReadMachineBuilds(state state.IState, input inputs_machiner.MachineBuildsInput) (any, error) {
	prefix := "vmBuilds::" + input.MachineId + "::"
	builds, err := state.Trx().GetLinksList(prefix, input.Offset, input.Count, false)
	if err != nil {
		log.Println(err)
		return nil, err
	}
	return map[string]any{"buildsList": builds}, nil
}

// Deploy /machines/deploy check [ true false false ] access [ true false false false POST ]
func (a *Actions) Deploy(state state.IState, input inputs_machiner.DeployInput) (any, error) {
	trx := state.Trx()
	if !trx.HasObj("Vm", input.MachineId) {
		return nil, errors.New("vm not found")
	}
	program := model.Program{MachineId: input.MachineId}.Pull(trx)
	if !trx.HasObj("App", program.AppId) {
		return nil, errors.New("app not found")
	}
	machine := model.Machine{Id: program.AppId}.Pull(trx)
	if machine.OwnerId != state.Info().UserId() {
		return nil, errors.New("access to vm denied")
	}
	if input.EntityType != "docker" && input.EntityType != "wasm" && input.EntityType != "elpify" && input.EntityType != "javascript" {
		return nil, errors.New("invalid entityType, expected one of docker|wasm|elpify|javascript")
	}
	data, err := base64.StdEncoding.DecodeString(input.Payload)
	if err != nil {
		return nil, err
	}
	entityPathForLink := ""
	entityModel := model.Entity{
		ProgramId:  program.AppId,
		EntityId:   input.EntityId,
		EntityType: input.EntityType,
		ImageName:  "",
	}
	if input.EntityType == "docker" {
		imageName := uuid.NewString()
		entityModel.ImageName = imageName
		files := map[string]any{}
		if input.Metadata != nil {
			filesRaw, ok := input.Metadata["files"]
			if ok {
				filesCast, ok2 := filesRaw.(map[string]any)
				if !ok2 {
					return nil, errors.New("files is not map")
				}
				files = filesCast
			}
		}
		dockerfileFolderPath := fmt.Sprintf("%s%s%s/entities/%s", a.App.Tools().Storage().StorageRoot(), pluginsTemplateName, program.MachineId, input.EntityId)
		err2 := a.App.Tools().File().SaveDataToGlobalStorage(dockerfileFolderPath, data, "Dockerfile", true)
		if err2 != nil {
			return nil, err2
		}
		for k, v := range files {
			dataStr, ok := v.(string)
			if !ok {
				err := errors.New("file bytecode not string")
				log.Println(err)
				return nil, err
			}
			data, err := base64.StdEncoding.DecodeString(dataStr)
			if err != nil {
				return nil, err
			}
			err2 := a.App.Tools().File().SaveDataToGlobalStorage(dockerfileFolderPath, data, k, true)
			if err2 != nil {
				return nil, err2
			}
		}
		buildId := uuid.NewString()
		trx.PutLink("vmBuilds::"+program.MachineId+"::"+buildId, "true")
		future.Async(func() {
			a.App.Tools().Vmm().BuildVmImage(program.MachineId, imageName, dockerfileFolderPath)
		}, false)
		entityPathForLink = dockerfileFolderPath + "/Dockerfile"
	} else {
		fileName := "module.wasm"
		if input.EntityType == "elpify" {
			fileName = "module.masm"
		} else if input.EntityType == "javascript" {
			fileName = "module.js"
		}
		entityFolderPath := fmt.Sprintf("%s%s%s/entities/%s", a.App.Tools().Storage().StorageRoot(), pluginsTemplateName, program.MachineId, input.EntityId)
		entityPath := entityFolderPath + "/" + fileName
		err2 := a.App.Tools().File().SaveDataToGlobalStorage(entityFolderPath, data, fileName, true)
		if err2 != nil {
			return nil, err2
		}
		program.Runtime = input.EntityType
		program.Path = entityPath
		entityPathForLink = entityPath
		program.Push(trx)
		if program.Runtime == "wasm" || program.Runtime == "elpify" || program.Runtime == "javascript" {
			a.App.Tools().Vmm().Assign(program.MachineId)
		} else if program.Runtime == "elpis" {
			a.App.Tools().Elpis().Assign(program.MachineId)
		}
	}
	entityModel.Push(trx)
	trx.PutLink("vmEntityPath::"+program.MachineId+"::"+input.EntityId, entityPathForLink)
	trx.PutLink("vmEntityType::"+program.MachineId+"::"+input.EntityId, input.EntityType)
	trx.PutLink("vmEntityDownloadable::"+program.MachineId+"::"+input.EntityId, strconv.FormatBool(input.Downloadable))
	return outputs_machiner.PlugInput{}, nil
}

// ListPrograms /programs/list check [ true false false ] access [ true false false false GET ]
func (a *Actions) ListPrograms(state state.IState, input inputs_machiner.ListInput) (any, error) {
	trx := state.Trx()
	machines, err := model.Machine{}.All(trx, input.Offset, input.Count)
	if err != nil {
		log.Println(err)
		return nil, err
	}
	result := []map[string]any{}
	for _, machine := range machines {
		profile, err := trx.GetJson("AppMeta::"+machine.Id, "metadata.public.profile")
		if err != nil {
			log.Println(err)
			result = append(result, map[string]any{
				"id":            machine.Id,
				"chainId":       machine.ChainId,
				"username":      machine.Username,
				"ownerId":       machine.OwnerId,
				"machinesCount": machine.MachinesCount,
				"title":         "untitled",
				"avatar":        "",
				"desc":          "",
			})
			continue
		}
		result = append(result, map[string]any{
			"id":            machine.Id,
			"chainId":       machine.ChainId,
			"username":      machine.Username,
			"ownerId":       machine.OwnerId,
			"machinesCount": machine.MachinesCount,
			"title":         profile["title"],
			"avatar":        profile["avatar"],
			"desc":          profile["desc"],
		})
	}
	return map[string]any{"apps": result}, nil
}

// ListMachines /machines/list check [ true false false ] access [ true false false false GET ]
func (a *Actions) ListMachines(state state.IState, input inputs_machiner.ListInput) (any, error) {
	trx := state.Trx()
	machines, err := model.User{}.All(trx, input.Offset, input.Count, map[string]string{"type": "machine"})
	if err != nil {
		log.Println(err)
		return nil, err
	}
	return map[string]any{"machines": machines}, nil
}

// ListProgramMachines /machines/listProgramMachines check [ true false false ] access [ true false false false GET ]
func (a *Actions) ListProgramMachines(state state.IState, input inputs_machiner.ListAppMachsInput) (any, error) {
	trx := state.Trx()
	machines, err := model.User{}.List(trx, "appMachines::"+input.AppId+"::", map[string]string{})
	if err != nil {
		log.Println(err)
		return nil, err
	}
	programs, err := model.Program{}.List(trx, "appMachines::"+input.AppId+"::")
	if err != nil {
		log.Println(err)
		return nil, err
	}
	programByMachineID := map[string]model.Program{}
	for _, program := range programs {
		programByMachineID[program.MachineId] = program
	}
	result := []map[string]any{}
	for _, machine := range machines {
		result = append(result, map[string]any{
			"id":       machine.Id,
			"type":     machine.Typ,
			"username": machine.Username,
			"runtime":  programByMachineID[machine.Id].Runtime,
			"path":     programByMachineID[machine.Id].Path,
			"comment":  programByMachineID[machine.Id].Comment,
		})
	}
	return map[string]any{"machines": result}, nil
}
