package inputs_stores

type SignalInput struct {
	Type       string `json:"type" validate:"required"`
	Data       string `json:"data" validate:"required"`
	StoreId    string `json:"storeId" validate:"required"`
	UserId     string `json:"userId"`
	Temp       bool   `json:"temp"`
	EditSignal bool   `json:"editSignal"`
}

func (d SignalInput) GetData() any {
	return "dummy"
}

func (d SignalInput) GetStoreId() string {
	return d.StoreId
}

func (d SignalInput) Origin() string {
	return ""
}
