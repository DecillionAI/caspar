package sdk

import (
	"fmt"
	"strings"
	"unsafe"
)

// AppEngine-compatible single host import.
//
//go:wasmimport env hostCall
func hostCall(offset uint32, length uint32) uint64

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

func call(op string, input string) string {
	return hostRequest(fmt.Sprintf(`{"op":"%s","input":%s}`, op, input))
}

func quote(v string) string {
	return `"` + strings.ReplaceAll(v, `"`, `\"`) + `"`
}

// ---- Full helper surface mirroring machines/wasm sdk host capabilities ----

func HttpPost(url string, headers string, body string) string {
	return call("httpPost", fmt.Sprintf(`{"url":%s,"headers":%s,"body":%s}`, quote(url), quote(headers), quote(body)))
}

func PlantTrigger(tag string, input string, pointID string, count int) string {
	return call("plantTrigger", fmt.Sprintf(`{"tag":%s,"input":%s,"pointId":%s,"count":%d}`, quote(tag), quote(input), quote(pointID), count))
}

func SignalPoint(typ string, pointID string, userID string, data string) string {
	return call("signalPoint", fmt.Sprintf(`{"type":%s,"pointId":%s,"userId":%s,"data":%s}`, quote(typ), quote(pointID), quote(userID), quote(data)))
}

func RunDocker(machineID string, pointID string, containerMeta string) string {
	return call("runVm", fmt.Sprintf(`{"machineId":%s,"pointId":%s,"containerMeta":%s,"vmType":"docker"}`, quote(machineID), quote(pointID), quote(containerMeta)))
}

func ExecDocker(machineID string, imageName string, containerName string, command string) string {
	return call("execVm", fmt.Sprintf(`{"machineId":%s,"imageName":%s,"containerName":%s,"command":%s}`, quote(machineID), quote(imageName), quote(containerName), quote(command)))
}

func CopyToDocker(machineID string, imageName string, containerName string, fileName string, content string) string {
	return call("copyToVm", fmt.Sprintf(`{"machineId":%s,"imageName":%s,"containerName":%s,"fileName":%s,"content":%s}`, quote(machineID), quote(imageName), quote(containerName), quote(fileName), quote(content)))
}

func Put(key string, val string) string {
	return call("dbOp", fmt.Sprintf(`{"op":"put","key":%s,"val":%s}`, quote(key), quote(val)))
}

func Del(key string) string {
	return call("dbOp", fmt.Sprintf(`{"op":"del","key":%s}`, quote(key)))
}

func Get(key string) string {
	return call("dbOp", fmt.Sprintf(`{"op":"get","key":%s}`, quote(key)))
}

func GetByPrefix(prefix string) string {
	return call("dbOp", fmt.Sprintf(`{"op":"getByPrefix","prefix":%s}`, quote(prefix)))
}

func ConsoleLog(text string) string {
	return call("consoleLog", fmt.Sprintf(`{"text":%s}`, quote(text)))
}

func SubmitOnchainTrx(targetMachineID string, key string, packet string, tag string, isFile bool, isBase bool) string {
	meta := "0"
	if isBase {
		meta = "1"
	}
	if isFile {
		meta += "1"
	} else {
		meta += "0"
	}
	meta += tag
	return call("submitOnchainTrx", fmt.Sprintf(`{"targetMachineId":%s,"key":%s,"packet":%s,"meta":%s}`,
		quote(targetMachineID), quote(key), quote(packet), quote(meta)))
}

func Output(text string) string {
	return call("output", fmt.Sprintf(`{"text":%s}`, quote(text)))
}

func NewSyncTask(name string, deps []string) string {
	quotedDeps := make([]string, 0, len(deps))
	for _, dep := range deps {
		quotedDeps = append(quotedDeps, quote(dep))
	}
	return call("newSyncTask", fmt.Sprintf(`{"name":%s,"deps":[%s]}`, quote(name), strings.Join(quotedDeps, ",")))
}

func CheckTokenValidity(token string) string {
	return call("checkTokenValidity", fmt.Sprintf(`{"token":%s}`, quote(token)))
}

func SendMessageOnChain(pointID string, payload string) string {
	return call("sendMessageOnChain", fmt.Sprintf(`{"pointId":%s,"payload":%s}`, quote(pointID), quote(payload)))
}

func RunVm(machineID string, input string, astPath string, vmType string) string {
	return call("runVm", fmt.Sprintf(`{"machineId":%s,"input":%s,"astPath":%s,"vmType":%s}`, quote(machineID), quote(input), quote(astPath), quote(vmType)))
}

func TerminateVm(machineID string) string {
	return call("terminateVm", fmt.Sprintf(`{"machineId":%s}`, quote(machineID)))
}

func LockResource(resourceID string, ownerID string) string {
	return call("lockResource", fmt.Sprintf(`{"runtime":"wasm","resourceId":%s,"ownerId":%s}`, quote(resourceID), quote(ownerID)))
}

func UnlockResource(resourceID string, ownerID string) string {
	return call("unlockResource", fmt.Sprintf(`{"runtime":"wasm","resourceId":%s,"ownerId":%s}`, quote(resourceID), quote(ownerID)))
}
