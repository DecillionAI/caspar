package inputs_storage

import (
	"kasper/src/shell/utils/origin"
)

type UploadDataInput struct {
	Data    string `json:"data" validate:"required"`
	StoreId string `json:"storeId" validate:"required"`
	FileId  string `json:"fileId"`
}

func (d UploadDataInput) GetData() any {
	return "dummy"
}

func (d UploadDataInput) GetStoreId() string {
	return d.StoreId
}

func (d UploadDataInput) Origin() string {
	return origin.FindOrigin(d.StoreId)
}
