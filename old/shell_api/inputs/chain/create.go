package inputs_machiner

type CreateInput struct {
	StoreId       *string `json:"storeId"`
	IsTemp        *bool   `json:"isTemp" validate:"required"`
	LockId        *string `json:"lockId"`
	LockSignature *string `json:"lockSignature"`
}

func (d CreateInput) GetData() any {
	return "dummy"
}

func (d CreateInput) GetStoreId() string {
	if d.StoreId != nil {
		return *d.StoreId
	}
	return ""
}

func (d CreateInput) Origin() string {
	return "global"
}
