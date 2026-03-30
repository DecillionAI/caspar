package inputs_machiner

type RunMachineInput struct {
	MachineId string            `json:"machineId" validate:"required"`
	EntityId  string            `json:"entityId" validate:"required"`
	Params    map[string]string `json:"params"`
}

func (d RunMachineInput) GetData() any {
	return "dummy"
}

func (d RunMachineInput) GetPointId() string {
	return ""
}

func (d RunMachineInput) Origin() string {
	return "global"
}
