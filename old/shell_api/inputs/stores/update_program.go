package inputs_stores

import "kasper/src/shell/utils/origin"

type UpdateProgramInput struct {
	StoreId     string      `json:"storeId" validate:"required"`
	MachineId   string      `json:"machineId" validate:"required"`
	ProgramMeta ProgramMeta `json:"programMeta" validate:"required"`
}

func (d UpdateProgramInput) GetData() any {
	return "dummy"
}

func (d UpdateProgramInput) GetStoreId() string {
	return d.StoreId
}

func (d UpdateProgramInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
