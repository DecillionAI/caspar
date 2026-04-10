package inputs_storage

type StreamGetInput struct {
	MachineId string `json:"machineId" validate:"required"`
	StoreId   string `json:"storeId" validate:"required"`
	Metadata  string `json:"metadata" validate:"required"`
}

func (d StreamGetInput) GetData() any {
	return "dummy"
}

func (d StreamGetInput) GetStoreId() string {
	return d.StoreId
}

func (d StreamGetInput) Origin() string {
	return ""
}
