package inputs_stores

import "kasper/src/shell/utils/origin"

type ListStoreMachinesInput struct {
	StoreId string `json:"storeId" validate:"required"`
}

func (d ListStoreMachinesInput) GetData() any {
	return "dummy"
}

func (d ListStoreMachinesInput) GetStoreId() string {
	return d.StoreId
}

func (d ListStoreMachinesInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
