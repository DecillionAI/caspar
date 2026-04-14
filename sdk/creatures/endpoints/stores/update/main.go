package main

import (
	"encoding/json"
	"unsafe"
)

//go:wasmimport env hostCall
func hostCall(offset uint32, length uint32) uint64

var heap = make([]byte, 1024*1024)
var heapPtr uint32 = 8

//export malloc
func malloc(size uint32) uint32 {
	ptr := heapPtr
	heapPtr += size
	return ptr
}

type packet struct {
	Payload    map[string]any `json:"payload"`
	CreatureID string         `json:"creatureId,omitempty"`
	SpaceID    string         `json:"spaceId,omitempty"`
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
		"key":   "Json::CreatureEndpoint::spaces::update",
		"path":  "lastInput",
		"data":  p.Payload,
		"merge": true,
	})
	packetBytes, _ := json.Marshal(map[string]any{"endpoint": "/spaces/update", "payload": p.Payload})
	signalKey := "spaces/update"
	if p.CreatureID != "" {
		hostReq("dbOp", map[string]any{"op": "put", "key": "creatureEndpoint::spaces::update::lastCreature", "val": p.CreatureID})
	}
	if p.SpaceID != "" {
		hostReq("signalGroup", map[string]any{"key": signalKey, "groupId": p.SpaceID, "packet": string(packetBytes), "system": true})
	}
	if p.CreatureID != "" {
		hostReq("signalUser", map[string]any{"key": signalKey, "userId": p.CreatureID, "creatureId": p.CreatureID, "packet": string(packetBytes), "system": true})
	}
	if targetCreatureID != "" && p.SpaceID != "" {
		hostReq("hasAccessToStore", map[string]any{"machineId": targetCreatureID, "targetCreatureId": targetCreatureID, "storeId": p.SpaceID})
	}
	out, _ := json.Marshal(map[string]any{"store": map[string]any{"admin": true}})
	hostReq("output", map[string]any{"text": string(out)})
	return string(out)
}

//export run
func run(arg uint64) int64 {
	input := stringAt(uint32(arg>>32), uint32(arg))
	process(input)
	return 0
}

func main() {}
