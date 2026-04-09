package actions_creature

import (
	"errors"
	"kasper/src/abstract/models/action"
	"kasper/src/abstract/models/core"
	"kasper/src/abstract/state"
	inputs_creatures "kasper/src/shell/api/inputs/creatures"
	inputs_users "kasper/src/shell/api/inputs/users"
	"kasper/src/shell/api/model"
	updates_points "kasper/src/shell/api/updates/points"
	"kasper/src/shell/utils/future"
)

type Actions struct {
	App           core.ICore
	modelExtender map[string]map[string]action.ExtendedField
}

func Install(a *Actions, params ...any) error {
	if len(params) >= 1 {
		a.modelExtender = params[0].(map[string]map[string]action.ExtendedField)
	} else {
		a.modelExtender = map[string]map[string]action.ExtendedField{}
	}
	return nil
}

// Create /creatures/create check [ false false false ] access [ true false false false POST ]
func (a *Actions) Create(state state.IState, input inputs_creatures.CreateInput) (any, error) {
	trx := state.Trx()
	creatureType := input.Type
	username := input.Username + "@" + state.Source()
	chainId := "main"
	subchainId := "main"
	ownerId := "free"
	if input.ChainId != nil && *input.ChainId != "" {
		chainId = *input.ChainId
	}
	if input.SubchainId != nil && *input.SubchainId != "" {
		subchainId = *input.SubchainId
	}
	if input.OwnerId != nil && *input.OwnerId != "" {
		ownerId = *input.OwnerId
	}
	if creatureType == "human" {
		chainId = "main"
		subchainId = "main"
		ownerId = "free"
	} else if ownerId == "free" {
		ownerId = state.Info().UserId()
	}
	if trx.HasIndex("Creature", "username", "id", username) {
		return nil, errors.New("creature username already exists")
	}
	balance := int64(0)
	if creatureType == "human" {
		balance = 1000000000000000
	}
	creature := model.Creature{
		Id:         a.App.Tools().Storage().GenId(trx, input.Origin()),
		TypeName:   creatureType,
		Username:   username,
		PublicKey:  input.PublicKey,
		ChainId:    chainId,
		SubchainId: subchainId,
		OwnerId:    ownerId,
		Balance:    balance,
	}
	creature.Push(trx)
	session := model.Session{Id: a.App.Tools().Storage().GenId(trx, input.Origin()), UserId: creature.Id}
	session.Push(trx)
	trx.PutJson("CreatMeta::"+creature.Id, "metadata", input.Metadata, false)
	if creatureType != "human" {
		trx.PutLink("ownerof::"+ownerId+"::"+creature.Id, "true")
	}
	return map[string]any{"creature": creature, "session": session}, nil
}

// Get /creatures/get check [ true false false ] access [ true false false false GET ]
func (a *Actions) Get(state state.IState, input inputs_users.GetInput) (any, error) {
	trx := state.Trx()
	if trx.HasObj("Creature", input.UserId) {
		return map[string]any{"creature": model.Creature{Id: input.UserId}.Pull(trx)}, nil
	}
	return nil, errors.New("creature not found")
}

// List /creatures/list check [ true false false ] access [ true false false false GET ]
func (a *Actions) List(state state.IState, input inputs_users.ListInput) (any, error) {
	creatures, err := model.Creature{}.All(state.Trx(), input.Offset, input.Count)
	if err != nil {
		return nil, err
	}
	return map[string]any{"creatures": creatures}, nil
}

// Transfer /creatures/transfer check [ true false false ] access [ true false false false POST ]
func (a *Actions) Transfer(state state.IState, input inputs_users.TransferInput) (any, error) {
	trx := state.Trx()
	from := model.Creature{Id: state.Info().UserId()}.Pull(trx)
	if from.Id == "" {
		return nil, errors.New("sender creature not found")
	}
	if from.Balance < input.Amount {
		return nil, errors.New("your balance is not enough")
	}
	toId := trx.GetIndex("Creature", "username", "id", input.ToUsername)
	if toId == "" {
		return nil, errors.New("target creature not found")
	}
	to := model.Creature{Id: toId}.Pull(trx)
	if to.Id == "" {
		return nil, errors.New("target creature not found")
	}
	from.Balance -= input.Amount
	to.Balance += input.Amount
	from.Push(trx)
	to.Push(trx)
	return map[string]any{}, nil
}

// Signal /creatures/signal check [ true false false ] access [ true false false false POST ]
func (a *Actions) Signal(state state.IState, input inputs_creatures.SignalInput) (any, error) {
	trx := state.Trx()
	senderCreature := model.Creature{Id: state.Info().UserId()}.Pull(trx)
	sender := model.User{Id: senderCreature.Id, Typ: senderCreature.TypeName, Username: senderCreature.Username, PublicKey: senderCreature.PublicKey}
	pointId := state.Info().PointId()
	if input.Type == "all" {
		if pointId == "" {
			return nil, errors.New("pointId is required for broadcast")
		}
		if trx.GetLink("onaccess::"+pointId+"::"+state.Info().UserId()) != "true" {
			return nil, errors.New("access denied")
		}
		packet := updates_points.Send{Action: "broadcast", User: sender, Data: input.Data, IsTemp: input.Temp}
		future.Async(func() {
			a.App.Tools().Signaler().SignalGroup("creatures/signal", pointId, packet, true, []string{state.Info().UserId()})
		}, false)
		return map[string]any{"passed": true}, nil
	}
	if input.Type != "pvp" {
		return nil, errors.New("unknown signal type")
	}
	if input.CreatureId == "" {
		return nil, errors.New("creatureId is required for pvp")
	}
	packet := updates_points.Send{Action: "single", User: sender, Data: input.Data, IsTemp: input.Temp}
	future.Async(func() {
		a.App.Tools().Signaler().SignalUser("creatures/signal", input.CreatureId, packet, true)
	}, false)
	return map[string]any{"passed": true}, nil
}
