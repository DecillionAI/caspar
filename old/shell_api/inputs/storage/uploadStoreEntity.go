package inputs_storage

import "kasper/src/shell/utils/origin"

type UploadStoreEntityInput struct {
	Data     string `json:"data" validate:"required"`
	StoreId string `json:"storeId" validate:"required"`
	EntityId string `json:"entityId" validate:"required"`
}

func (d UploadStoreEntityInput) GetData() any {
	return "dummy"
}

func (d UploadStoreEntityInput) GetStoreId() string {
	return d.StoreId
}

func (d UploadStoreEntityInput) Origin() string {
	return origin.FindOrigin(d.StoreId)
}
