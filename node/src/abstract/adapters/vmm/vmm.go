package vmm

import "kasper/src/abstract/models/worker"

type IVmm interface {
	Assign(machineId string)
	RunVm(machineId string, pointId string, data string)
	TerminateVm(machineId string)
	BuildVmImage(machineId string, imageName string, dockerfilePath string)
	ExecuteChainTrxsGroup(trxs []*worker.Trx)
	ExecuteChainEffects(effects string)
	CloseKVDB()
	VmCallback(dataRaw string) (string, int64)
}
