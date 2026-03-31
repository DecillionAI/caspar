package inputs_machiner

type RunProgramEntityInput struct {
	ProgramId string            `json:"programId"`
	MachineId string            `json:"machineId"`
	EntityId  string            `json:"entityId" validate:"required"`
	VmId      string            `json:"vmId"`
	Params    map[string]string `json:"params"`
}

func (d RunProgramEntityInput) GetData() any {
	return "dummy"
}

func (d RunProgramEntityInput) GetPointId() string {
	return ""
}

func (d RunProgramEntityInput) Origin() string {
	return "global"
}
