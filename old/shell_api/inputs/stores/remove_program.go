package inputs_stores

import "kasper/src/shell/utils/origin"

type RemoveProgramInput struct {
	MachineId  string `json:"machineId" validate:"required"`
	StoreId    string `json:"storeId" validate:"required"`
	ProgramId  string `json:"programId" validate:"required"`
	Identifier string `json:"identifier" validate:"required"`
}

func (d RemoveProgramInput) GetData() any {
	return "dummy"
}

func (d RemoveProgramInput) GetStoreId() string {
	return d.StoreId
}

func (d RemoveProgramInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
