package vmm

import "encoding/json"

// VmCallback handles appengine host-call packets globally for all appengine
// runtimes (wasm/docker/javascript/elpify/elpian) through the same ZeroMQ callback channel.
func (wm *Vmm) VmCallback(dataRaw string) (string, int64) {
	println(dataRaw)
	data := map[string]any{}
	err := json.Unmarshal([]byte(dataRaw), &data)
	if err != nil {
		println(err)
		return err.Error(), 0
	}
	reqIdRaw, err := checkField(data, "requestId", float64(0))
	if err != nil {
		println(err)
		return err.Error(), 0
	}
	reqId := int64(reqIdRaw)
	key, err := checkField(data, "key", "")
	if err != nil {
		println(err)
		return err.Error(), reqId
	}
	input, err := checkField[map[string]any](data, "input", nil)
	if err != nil {
		println(err)
		return err.Error(), reqId
	}

	switch key {
	case "execDocker", "execVm":
		return wm.handleExecDocker(input, reqId)
	case "copyToDocker", "copyToVm":
		return wm.handleCopyToDocker(input, reqId)
	case "checkTokenValidity":
		return wm.handleCheckTokenValidity(input, reqId)
	case "plantTrigger":
		return wm.handlePlantTrigger(input, reqId)
	case "signalPoint":
		return wm.handleSignalPoint(input, reqId)
	case "runVm":
		return wm.handleRunVM(input, reqId)
	case "terminateVm":
		return wm.handleTerminateVM(input, reqId)
	case "sendMessageOnChain":
		return wm.handleSendMessageOnChain(input, reqId)
	case "createCreature":
		return wm.handleCreatureCrud("create", input, reqId)
	case "updateCreature":
		return wm.handleCreatureCrud("update", input, reqId)
	case "deleteCreature":
		return wm.handleCreatureCrud("delete", input, reqId)
	case "getCreature":
		return wm.handleCreatureCrud("get", input, reqId)
	case "listCreatures":
		return wm.handleCreatureCrud("list", input, reqId)
	case "createResourceStore", "createVmOwnedStore":
		return wm.handleResourceStoreCrud("create", input, reqId)
	case "updateResourceStore", "updateVmOwnedStore":
		return wm.handleResourceStoreCrud("update", input, reqId)
	case "deleteResourceStore", "deleteVmOwnedStore":
		return wm.handleResourceStoreCrud("delete", input, reqId)
	case "getResourceStore", "getVmOwnedStore":
		return wm.handleResourceStoreCrud("get", input, reqId)
	case "listResourceStores", "listVmOwnedStores":
		return wm.handleResourceStoreCrud("list", input, reqId)
	case "createResourceEntity":
		return wm.handleResourceEntityCreate(input, reqId)
	case "deleteResourceEntity":
		return wm.handleResourceEntityDelete(input, reqId)
	case "createWorkchain":
		return wm.handleVmChainRequest("createWorkchain", input, reqId)
	case "deleteWorkchain":
		return wm.handleVmChainRequest("deleteWorkchain", input, reqId)
	case "createSubchain":
		return wm.handleVmChainRequest("createSubchain", input, reqId)
	case "deleteSubchain":
		return wm.handleVmChainRequest("deleteSubchain", input, reqId)
	case "execShellAction":
		return wm.handleExecShellAction(input, reqId)
	case "microGenId":
		return wm.handleMicroHostAction("genId", input, reqId)
	case "microGetLink":
		return wm.handleMicroHostAction("getLink", input, reqId)
	case "microPutLink":
		return wm.handleMicroHostAction("putLink", input, reqId)
	case "microDelKey":
		return wm.handleMicroHostAction("delKey", input, reqId)
	case "microGetJson":
		return wm.handleMicroHostAction("getJson", input, reqId)
	case "microPutJson":
		return wm.handleMicroHostAction("putJson", input, reqId)
	case "microGetByPrefix":
		return wm.handleMicroHostAction("getByPrefix", input, reqId)
	case "microHasAccessToPoint":
		return wm.handleMicroHostAction("hasAccessToPoint", input, reqId)
	case "microSignalUser":
		return wm.handleMicroHostAction("signalUser", input, reqId)
	case "microSignalGroup":
		return wm.handleMicroHostAction("signalGroup", input, reqId)
	case "microJoinGroup":
		return wm.handleMicroHostAction("joinGroup", input, reqId)
	case "log":
		_, err := checkField(input, "text", "")
		if err != nil {
			println(err)
			return err.Error(), reqId
		}
	}

	return "{}", reqId
}
