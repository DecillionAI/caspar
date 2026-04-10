package main

import (
	"encoding/json"
	"unsafe"
)

//go:wasmimport env hostCall
func hostCall(offset uint32, length uint32) uint64

var retBuf []byte

type packet struct {
	Payload   map[string]any `json:"payload"`
	UserID    string         `json:"userId,omitempty"`
	SpaceID   string         `json:"spaceId,omitempty"`
	MachineID string         `json:"machineId,omitempty"`
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

func hostReq(op string, input map[string]any) string {
	req := map[string]any{"op": op, "input": input}
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
	if "addMachine" == "create" || "addMachine" == "createFromSpace" || "addMachine" == "createShard" {
		hostReq("genId", map[string]any{"source": "spaces.addMachine"})
	}
	hostReq("putJson", map[string]any{
		"key":   "Json::CreatureEndpoint::spaces::addMachine",
		"path":  "lastInput",
		"data":  p.Payload,
		"merge": true,
	})
	if p.UserID != "" {
		hostReq("dbOp", map[string]any{"op": "put", "key": "creatureEndpoint::spaces::addMachine::lastUser", "val": p.UserID})
	}
	if p.SpaceID != "" {
		hostReq("signalGroup", map[string]any{"key": "creatures/signal", "groupId": p.SpaceID, "packet": "{}", "system": true})
	}
	if p.UserID != "" {
		hostReq("signalUser", map[string]any{"key": "creatures/signal", "userId": p.UserID, "packet": "{}", "system": true})
	}
	if p.MachineID != "" && p.SpaceID != "" {
		hostReq("hasAccessToStore", map[string]any{"machineId": p.MachineID, "storeId": p.SpaceID})
	}
	out, _ := json.Marshal(map[string]any{"ok": true, "endpoint": "/spaces/addMachine"})
	hostReq("output", map[string]any{"text": string(out)})
	return string(out)
}

//export run
func run(inputPtr int32, inputLen int32) int32 {
	input := stringAt(uint32(inputPtr), uint32(inputLen))
	return makeReturn(process(input))
}

func main() {}
