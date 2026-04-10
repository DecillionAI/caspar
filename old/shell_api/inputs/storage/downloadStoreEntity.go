package inputs_storage

type DownloadStoreEntityInput struct {
	EntityId string `json:"entityId" validate:"required"`
	StoreId  string `json:"storeId" validate:"required"`
}

func (d DownloadStoreEntityInput) GetData() any {
	return "dummy"
}

func (d DownloadStoreEntityInput) GetStoreId() string {
	return d.StoreId
}

func (d DownloadStoreEntityInput) Origin() string {
	return ""
}
