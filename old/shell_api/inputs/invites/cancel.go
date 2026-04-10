package inputs_invites

import "kasper/src/shell/utils/origin"

type CancelInput struct {
	StoreId string `json:"storeId" validate:"required"`
	UserId  string `json:"userId" validate:"required"`
}

func (d CancelInput) GetData() any {
	return "dummy"
}

func (d CancelInput) GetStoreId() string {
	return d.StoreId
}

func (d CancelInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
