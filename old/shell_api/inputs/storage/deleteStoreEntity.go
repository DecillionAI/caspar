package inputs_storage

import "kasper/src/shell/utils/origin"

type DeleteStoreEntityInput struct {
	StoreId string `json:"storeId" validate:"required"`
	EntityId string `json:"entityId" validate:"required"`
}

func (d DeleteStoreEntityInput) GetData() any {
	return "dummy"
}

func (d DeleteStoreEntityInput) GetStoreId() string {
	return d.StoreId
}

func (d DeleteStoreEntityInput) Origin() string {
	return origin.FindOrigin(d.StoreId)
}
