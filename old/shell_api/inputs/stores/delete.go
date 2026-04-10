package inputs_stores

import "kasper/src/shell/utils/origin"

type DeleteInput struct {
	StoreId string `json:"storeId" validate:"required"`
}

func (d DeleteInput) GetData() any {
	return "dummy"
}

func (d DeleteInput) GetStoreId() string {
	return d.StoreId
}

func (d DeleteInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
