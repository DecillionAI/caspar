package inputs_stores

import "kasper/src/shell/utils/origin"

type AddProgramInput struct {
	MachineId   string      `json:"machineId" validate:"required"`
	StoreId     string      `json:"storeId" validate:"required"`
	ProgramMeta ProgramMeta `json:"programMeta" validate:"required"`
}

func (d AddProgramInput) GetData() any {
	return "dummy"
}

func (d AddProgramInput) GetStoreId() string {
	return d.StoreId
}

func (d AddProgramInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
