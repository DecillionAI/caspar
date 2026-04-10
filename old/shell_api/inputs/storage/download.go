package inputs_storage

type DownloadInput struct {
	FileId  string `json:"fileId" validate:"required"`
	StoreId string `json:"storeId" validate:"required"`
}

func (d DownloadInput) GetData() any {
	return "dummy"
}

func (d DownloadInput) GetStoreId() string {
	return d.StoreId
}

func (d DownloadInput) Origin() string {
	return ""
}
