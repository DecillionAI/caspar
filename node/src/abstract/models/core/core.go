package core

import (
	"kasper/src/abstract/adapters/tools"
	"kasper/src/abstract/models/action"
	"kasper/src/abstract/models/chain"
	"kasper/src/abstract/models/info"
	"kasper/src/abstract/models/trx"
	"kasper/src/abstract/models/update"
	"kasper/src/abstract/state"
)

type EmptyPayload struct{}

type ResponseHolder struct {
	Payload []byte
	Effects chain.Effects
}

type ICore interface {
	OwnerId() string
	Id() string
	Gods() []string
	AddGod(username string)
	Tools() tools.ITools
	FreeNodes() map[string]bool
	AddFreeNode(nodeId string)
	Actor() action.IActor
	Load([]string, map[string]interface{})
	Close()
	PlantChainTrigger(count int, userId string, tag string, machineId string, pointId string, input string)
	ExecBaseRequestOnChain(key string, payload []byte, signature string, userId string, tag string, callback func([]byte, int, error))
	SendMessageOnChain(key string, payload []byte, signature string, userId string, receivers map[string]map[string]bool, replyTo string, callback func(string, []byte))
	SendTypedMessageOnChain(chainId string, key string, messageType string, payload []byte, signature string, userId string, receivers map[string]map[string]bool, replyTo string, pointId string, pay *chain.ChainPayPacket, callback func(string, []byte))
	ExecBaseResponseOnChain(callbackId string, packet []byte, signature string, resCode int, e string, updates []update.Update, tag string, toUserId string)
	OnChainPacket(typ string, trxPayload []byte) string
	AppPendingTrxs()
	IpAddr() string
	ModifyState(bool, func(trx.ITrx) error)
	ModifyStateSecurlyWithSource(readonly bool, info info.IInfo, src string, fn func(state.IState) error)
	ModifyStateSecurly(readonly bool, info info.IInfo, fn func(state.IState) error)
	SignPacket(data []byte) string
	SignPacketAsOwner(data []byte) string
	ExecutionCostPerSecond() int64
}
