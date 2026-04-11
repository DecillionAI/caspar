package main

import (
	"encoding/json"
	"unsafe"
)

//go:wasmimport env hostCall
func hostCall(offset uint32, length uint32) uint64

var retBuf []byte

type packet struct {
	Payload    map[string]any `json:"payload"`
	CreatureID string         `json:"creatureId,omitempty"`
	StoreID    string         `json:"storeId,omitempty"`
}

func bytesAt(offset uint32, length uint32) []byte {
	return unsafe.Slice((*byte)(unsafe.Pointer(uintptr(offset))), length)
}

func stringAt(offset uint32, length uint32) string {
	return string(bytesAt(offset, length))
}

func hostRequest(req string) string {
	ptr := uint32(uintptr(unsafe.Pointer(unsafe.StringData(req))))
	ret := hostCall(ptr, uint32(len(req)))
	retOffset := uint32(ret >> 32)
	retLen := uint32(ret)
	return stringAt(retOffset, retLen)
}

var hostCreatureID string
var hostProgramID string
var hostEntityName string
var hostEntityPath string

func extractContextString(input map[string]any, keys ...string) string {
	if input == nil {
		return ""
	}
	for _, key := range keys {
		if v, ok := input[key].(string); ok && v != "" {
			return v
		}
	}
	return ""
}

func setHostContext(creatureID string, payload map[string]any) {
	hostCreatureID = creatureID
	if hostCreatureID == "" {
		hostCreatureID = extractContextString(payload, "creatureId", "userId")
	}

	hostProgramID = extractContextString(payload, "programId", "targetCreatureId", "machineId")
	if hostProgramID == "" {
		hostProgramID = hostCreatureID
	}

	hostEntityName = extractContextString(payload, "entityId", "entityName", "entity", "name")
	hostEntityPath = extractContextString(payload, "entityPath", "astPath", "astpath", "path")
}

func hostReq(op string, input map[string]any) string {
	creatureID := hostCreatureID
	programID := hostProgramID
	entityName := hostEntityName
	entityPath := hostEntityPath
	if input != nil {
		if v := extractContextString(input, "creatureId", "userId"); v != "" {
			creatureID = v
		}
		if v := extractContextString(input, "programId", "targetCreatureId", "machineId"); v != "" {
			programID = v
		}
		if v := extractContextString(input, "entityId", "entityName", "entity", "name"); v != "" {
			entityName = v
		}
		if v := extractContextString(input, "entityPath", "astPath", "astpath", "path"); v != "" {
			entityPath = v
		}
	}

	if programID == "" {
		programID = "system"
	}

	req := map[string]any{
		"creatureId": creatureID,
		"programId":  programID,
		"entityId":   entityName,
		"entityPath": entityPath,
		"op":         op,
		"input":      input,
	}
	b, _ := json.Marshal(req)
	return hostRequest(string(b))
}

func makeReturn(s string) int32 {
	retBuf = append([]byte(s), 0)
	return int32(uintptr(unsafe.Pointer(&retBuf[0])))
}

func process(input string) string {
	p := packet{}
	if input != "" {
		_ = json.Unmarshal([]byte(input), &p)
	}
	setHostContext(p.CreatureID, p.Payload)
	targetCreatureID, _ := p.Payload["targetCreatureId"].(string)
	if targetCreatureID == "" {
		targetCreatureID, _ = p.Payload["machineId"].(string)
	}
	hostReq("putJson", map[string]any{
		"key":   "Json::CreatureEndpoint::chains::registerNode",
		"path":  "lastInput",
		"data":  p.Payload,
		"merge": true,
	})
	packetBytes, _ := json.Marshal(map[string]any{"endpoint": "/chains/registerNode", "payload": p.Payload})
	signalKey := "chains/registerNode"
	if p.CreatureID != "" {
		hostReq("dbOp", map[string]any{"op": "put", "key": "creatureEndpoint::chains::registerNode::lastCreature", "val": p.CreatureID})
	}
	if p.StoreID != "" {
		hostReq("signalGroup", map[string]any{"key": signalKey, "groupId": p.StoreID, "packet": string(packetBytes), "system": true})
	}
	if p.CreatureID != "" {
		hostReq("signalUser", map[string]any{"key": signalKey, "userId": p.CreatureID, "creatureId": p.CreatureID, "packet": string(packetBytes), "system": true})
	}
	if targetCreatureID != "" && p.StoreID != "" {
		hostReq("hasAccessToStore", map[string]any{"machineId": targetCreatureID, "targetCreatureId": targetCreatureID, "storeId": p.StoreID})
	}
	out, _ := json.Marshal(map[string]any{})
	hostReq("output", map[string]any{"text": string(out)})
	return string(out)
}

//export run
func run(inputPtr int32, inputLen int32) int32 {
	input := stringAt(uint32(inputPtr), uint32(inputLen))
	return makeReturn(process(input))
}

func main() {}
