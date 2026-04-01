package actions_chain

import (
	"encoding/json"
	"errors"
	"kasper/src/abstract/models/core"
	"kasper/src/abstract/state"
	inputs_chain "kasper/src/shell/api/inputs/chain"
	inputs_users "kasper/src/shell/api/inputs/users"
	"kasper/src/shell/utils/origin"
	"net"
	"strings"
)

func asInt64(raw any) (int64, bool) {
	switch v := raw.(type) {
	case int:
		return int64(v), true
	case int64:
		return v, true
	case float64:
		return int64(v), true
	default:
		return 0, false
	}
}

type Actions struct {
	App core.ICore
}

func Install(a *Actions, extra ...any) error {
	return nil
}

func (a *Actions) consumeChainCreationLock(state state.IState, lockId *string, lockSignature *string) error {
	if state.Info().UserId() == a.App.OwnerId() {
		return nil
	}
	if lockId == nil || lockSignature == nil || *lockId == "" || *lockSignature == "" {
		return nil
	}
	payment, err := state.Trx().GetJson("Json::User::"+state.Info().UserId(), "lockedTokens."+*lockId)
	if err != nil {
		return errors.New("lock not found")
	}
	typ, _ := payment["type"].(string)
	stepsRaw, ok := payment["steps"].([]any)
	if typ != "pay" || !ok || len(stepsRaw) == 0 {
		return errors.New("invalid lock payment")
	}
	firstStep, ok := stepsRaw[0].(map[string]any)
	if !ok {
		return errors.New("invalid lock payment step")
	}
	amountRaw, ok := asInt64(firstStep["amount"])
	if !ok || amountRaw <= 0 {
		return errors.New("invalid lock payment step amount")
	}
	step := 0
	payload, err := json.Marshal(inputs_users.ConsumeLockInput{
		Type:      "pay",
		UserId:    state.Info().UserId(),
		LockId:    *lockId,
		Signature: *lockSignature,
		Amount:    amountRaw,
		Step:      &step,
	})
	if err != nil {
		return err
	}
	lockConsumeDone := make(chan error, 1)
	a.App.ExecBaseRequestOnChain("/users/consumeLock", payload, a.App.SignPacketAsOwner(payload), a.App.OwnerId(), "", func(b []byte, i int, err error) {
		if err != nil {
			lockConsumeDone <- err
			return
		}
		if i >= 400 {
			lockConsumeDone <- errors.New("failed to consume lock")
			return
		}
		lockConsumeDone <- nil
	})
	return <-lockConsumeDone
}

// Create /chains/create check [ true false false ] access [ true false false false POST ]
func (a *Actions) Create(state state.IState, input inputs_chain.CreateInput) (any, error) {
	if err := a.consumeChainCreationLock(state, input.LockId, input.LockSignature); err != nil {
		return nil, err
	}
	id := ""
	pointId := ""
	if input.PointId != nil {
		pointId = *input.PointId
	}
	if *input.IsTemp {
		id = a.App.Tools().Network().Chain().CreateTempChain(pointId)
	} else {
		id = a.App.Tools().Network().Chain().CreateWorkChain(pointId)
	}
	return map[string]any{"chainId": id, "pointId": pointId}, nil
}

// CreateShard /chains/createShard check [ true false false ] access [ true false false false POST ]
func (a *Actions) CreateShard(state state.IState, input inputs_chain.CreateShardInput) (any, error) {
	if err := a.consumeChainCreationLock(state, input.LockId, input.LockSignature); err != nil {
		return nil, err
	}
	shardChainId := ""
	if input.ShardChainId != nil {
		shardChainId = *input.ShardChainId
	}
	id := a.App.Tools().Network().Chain().CreateShardChain(input.ChainId, shardChainId, input.Peers)
	if id == "" {
		return nil, errors.New("work chain not found")
	}
	return map[string]any{"chainId": input.ChainId, "shardChainId": id}, nil
}

// CreateFromPoint /chains/createFromPoint check [ true true false ] access [ true false false false POST ]
func (a *Actions) CreateFromPoint(state state.IState, input inputs_chain.CreateFromPointInput) (any, error) {
	members, err := state.Trx().GetLinksList("member::"+state.Info().PointId()+"::", -1, -1)
	if err != nil {
		return nil, err
	}
	peersMap := map[string]bool{}
	for _, memberLink := range members {
		parts := strings.Split(memberLink, "::")
		if len(parts) < 3 {
			continue
		}
		memberId := parts[len(parts)-1]
		addr := origin.FindOrigin(memberId)
		if addr != "" {
			peersMap[addr] = true
		}
	}
	peers := []string{}
	for addr := range peersMap {
		peers = append(peers, addr)
	}

	createResRaw, err := a.Create(state, inputs_chain.CreateInput{PointId: &input.PointId, IsTemp: input.IsTemp, LockId: input.LockId, LockSignature: input.LockSignature})
	if err != nil {
		return nil, err
	}
	createRes, ok := createResRaw.(map[string]any)
	if !ok {
		return nil, errors.New("failed to create work chain")
	}
	chainId, _ := createRes["chainId"].(string)
	if chainId == "" {
		return nil, errors.New("failed to create work chain")
	}

	shardChainId := a.App.Tools().Network().Chain().CreateShardChain(chainId, "", peers)
	if shardChainId == "" {
		return nil, errors.New("work chain not found")
	}

	return map[string]any{
		"chainId":      chainId,
		"shardChainId": shardChainId,
		"peers":        peers,
		"pointId":      input.PointId,
	}, nil
}

// SubmitBaseTrx /chains/submitBaseTrx check [ true false false ] access [ true false false false POST ]
func (a *Actions) SubmitBaseTrx(state state.IState, input inputs_chain.SubBaseTrxInput) (any, error) {
	var result []byte
	var resErr error
	a.App.ExecBaseRequestOnChain(input.Key, input.Payload, input.Signature, state.Info().UserId(), "", func(b []byte, i int, err error) {
		result = b
		resErr = err
	})
	if resErr != nil {
		return nil, resErr
	}
	res := map[string]any{}
	e := json.Unmarshal(result, &res)
	if e != nil {
		return nil, e
	}
	return res, nil
}

// RegisterNode /chains/registerNode check [ true false false ] access [ true false false false POST ]
func (a *Actions) RegisterNode(state state.IState, input inputs_chain.RegisterNodeInput) (any, error) {
	ipAddr := ""
	ips, _ := net.LookupIP(input.Orig)
	for _, ip := range ips {
		if ipv4 := ip.To4(); ipv4 != nil {
			ipAddr = ipv4.String()
			break
		}
	}
	state.Trx().PutLink("NodeIpToHost::"+ipAddr, input.Orig)
	return map[string]any{}, nil
}
