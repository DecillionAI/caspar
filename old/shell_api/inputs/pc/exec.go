package inputs_pc

type ExecCommandInput struct {
	VmId    string `json:"vmId" validate:"required"`
	Command string `json:"command" validate:"required"`
}

func (d ExecCommandInput) GetData() any {
	return "dummy"
}

func (d ExecCommandInput) GetStoreId() string {
	return ""
}

func (d ExecCommandInput) Origin() string {
	return ""
}
