package inputs_stores

import "kasper/src/shell/utils/origin"

type JoinInput struct {
	StoreId string `json:"storeId" validate:"required"`
}

func (d JoinInput) GetData() any {
	return "dummy"
}

func (d JoinInput) GetStoreId() string {
	return d.StoreId
}

func (d JoinInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
