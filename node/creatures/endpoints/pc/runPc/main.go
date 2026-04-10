package main

import (
	"encoding/json"
	"fmt"
	"strings"
	"unsafe"
)

//go:wasmimport env hostCall
func hostCall(offset uint32, length uint32) uint64

var retBuf []byte

type packet struct {
	Payload   map[string]any `json:"payload"`
	UserID    string         `json:"userId,omitempty"`
	StoreID   string         `json:"storeId,omitempty"`
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

	// Forward streamed firecracker output events to the original requester.
	if p.Payload["event"] == "fireVmOutput" {
		requesterUserID, _ := p.Payload["requesterUserId"].(string)
		if requesterUserID != "" {
			streamPayload, _ := json.Marshal(map[string]any{
				"type":   "pc.fire.stream",
				"source": "pc.runPc",
				"data":   p.Payload,
			})
			hostReq("signalUser", map[string]any{
				"key":    "creatures/signal",
				"userId": requesterUserID,
				"packet": string(streamPayload),
				"system": true,
			})
		}
		out, _ := json.Marshal(map[string]any{"ok": true, "forwarded": true, "endpoint": "/pc/runPc"})
		return string(out)
	}

	vmID := ""
	if v, ok := p.Payload["vmId"].(string); ok && v != "" {
		vmID = v
	} else {
		genRes := hostReq("genId", map[string]any{"source": "pc.runPc.vm"})
		genMap := map[string]any{}
		_ = json.Unmarshal([]byte(genRes), &genMap)
		vmID, _ = genMap["id"].(string)
		if vmID == "" {
			vmID = "main"
		}
	}

	command := ""
	if v, ok := p.Payload["command"].(string); ok {
		command = strings.TrimSpace(v)
	}
	runInput := map[string]any{
		"runtime":         "fire",
		"machineId":       p.MachineID,
		"storeId":         p.StoreID,
		"requesterUserId": p.UserID,
		"vmId":            vmID,
		"standalone":      true,
	}
	if command != "" {
		runInput["data"] = fmt.Sprintf("{\"command\":%q}", command)
	}
	hostReq("runVm", runInput)

	hostReq("putJson", map[string]any{
		"key":   "Json::CreatureEndpoint::pc::runPc",
		"path":  "lastFireRun",
		"data":  map[string]any{"machineId": p.MachineID, "storeId": p.StoreID, "requesterUserId": p.UserID, "vmId": vmID, "command": command},
		"merge": true,
	})

	out, _ := json.Marshal(map[string]any{"ok": true, "endpoint": "/pc/runPc", "runtime": "fire", "vmId": vmID})
	hostReq("output", map[string]any{"text": string(out)})
	return string(out)
}

//export run
func run(inputPtr int32, inputLen int32) int32 {
	input := stringAt(uint32(inputPtr), uint32(inputLen))
	return makeReturn(process(input))
}

func main() {}
