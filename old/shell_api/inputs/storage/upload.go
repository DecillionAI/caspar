package inputs_storage

import (
	"mime/multipart"
)

type UploadInput struct {
	Data    *multipart.FileHeader `json:"data" validate:"required"`
	StoreId string                `json:"storeId" validate:"required"`
	FileId  string                `json:"fileId"`
}

func (d UploadInput) GetData() any {
	return "dummy"
}

func (d UploadInput) GetStoreId() string {
	return d.StoreId
}

func (d UploadInput) Origin() string {
	return ""
}
