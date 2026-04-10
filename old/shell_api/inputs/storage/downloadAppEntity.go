package inputs_storage

type DownloadAppEntityInput struct {
	EntityId string `json:"entityId" validate:"required"`
	AppId    string `json:"appId" validate:"required"`
}

func (d DownloadAppEntityInput) GetData() any {
	return "dummy"
}

func (d DownloadAppEntityInput) GetStoreId() string {
	return ""
}

func (d DownloadAppEntityInput) Origin() string {
	return ""
}
