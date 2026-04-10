package inputs_machiner

import "kasper/src/shell/utils/origin"

type CreateFromStoreInput struct {
	StoreId       string  `json:"storeId" validate:"required"`
	IsTemp        *bool   `json:"isTemp" validate:"required"`
	LockId        *string `json:"lockId"`
	LockSignature *string `json:"lockSignature"`
}

func (d CreateFromStoreInput) GetData() any {
	return "dummy"
}

func (d CreateFromStoreInput) GetStoreId() string {
	return d.StoreId
}

func (d CreateFromStoreInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
