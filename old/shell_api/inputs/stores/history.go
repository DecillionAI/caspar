package inputs_stores

type HistoryInput struct {
	StoreId    string   `json:"storeId" validate:"required"`
	Count      int      `json:"count" validate:"required"`
	BeforeTime int64    `json:"beforeTime"`
	Ids        []string `json:"Ids"`
}

func (d HistoryInput) GetData() any {
	return "dummy"
}

func (d HistoryInput) GetStoreId() string {
	return d.StoreId
}

func (d HistoryInput) Origin() string {
	return ""
}
