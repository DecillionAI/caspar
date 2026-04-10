package inputs_machiner

type ReadBuildLogsInput struct {
	MachineId string `json:"machineId" validate:"required"`
	BuildId   string `json:"buildId" validate:"required"`
}

func (d ReadBuildLogsInput) GetData() any {
	return "dummy"
}

func (d ReadBuildLogsInput) GetStoreId() string {
	return ""
}

func (d ReadBuildLogsInput) Origin() string {
	return "global"
}
