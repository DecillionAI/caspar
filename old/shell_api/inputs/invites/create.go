package inputs_invites

import "kasper/src/shell/utils/origin"

type CreateInput struct {
	StoreId string `json:"storeId" validate:"required"`
	UserId  string `json:"userId" validate:"required"`
}

func (d CreateInput) GetData() any {
	return "dummy"
}

func (d CreateInput) GetStoreId() string {
	return d.StoreId
}

func (d CreateInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
