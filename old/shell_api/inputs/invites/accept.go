package inputs_invites

import "kasper/src/shell/utils/origin"

type AcceptInput struct {
	StoreId string `json:"storeId" validate:"required"`
}

func (d AcceptInput) GetData() any {
	return "dummy"
}

func (d AcceptInput) GetStoreId() string {
	return ""
}

func (d AcceptInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
