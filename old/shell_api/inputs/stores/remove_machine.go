package inputs_stores

import "kasper/src/shell/utils/origin"

type RemoveMachineInput struct {
	MachineId string `json:"machineId" validate:"required"`
	StoreId   string `json:"storeId" validate:"required"`
}

func (d RemoveMachineInput) GetData() any {
	return "dummy"
}

func (d RemoveMachineInput) GetStoreId() string {
	return d.StoreId
}

func (d RemoveMachineInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
