package inputs_stores

type GlobalHistoryInput struct {
	Count int      `json:"count" validate:"required"`
	Ids   []string `json:"Ids" validate:"required"`
}

func (d GlobalHistoryInput) GetData() any {
	return "dummy"
}

func (d GlobalHistoryInput) GetStoreId() string {
	return ""
}

func (d GlobalHistoryInput) Origin() string {
	return ""
}
