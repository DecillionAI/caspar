package inputs_pc

type RunPcInput struct{}

func (d RunPcInput) GetData() any {
	return "dummy"
}

func (d RunPcInput) GetStoreId() string {
	return ""
}

func (d RunPcInput) Origin() string {
	return ""
}
