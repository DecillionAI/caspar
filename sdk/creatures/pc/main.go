package main

import (
	"encoding/json"
	"unsafe"
)

//go:wasmimport env hostCall
func hostCall(offset uint32, length uint32) uint64

var retBuf []byte

type packet struct {
	Path    string         `json:"path"`
	Payload map[string]any `json:"payload"`
	UserID  string         `json:"userId,omitempty"`
	StoreID string         `json:"storeId,omitempty"`
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
	hostReq("putJson", map[string]any{"key": "Json::CreatureNamespace::pc", "path": "lastInput", "data": p.Payload, "merge": true})
	if p.Path != "" {
		hostReq("dbOp", map[string]any{"op": "put", "key": "creatureNamespace::pc::lastPath", "val": p.Path})
	}
	out, _ := json.Marshal(map[string]any{"ok": true, "namespace": "pc"})
	hostReq("output", map[string]any{"text": string(out)})
	return string(out)
}

//export run
func run(inputPtr int32, inputLen int32) int32 {
	input := stringAt(uint32(inputPtr), uint32(inputLen))
	return makeReturn(process(input))
}

func main() {}
