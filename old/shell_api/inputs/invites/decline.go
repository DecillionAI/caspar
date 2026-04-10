package inputs_invites

import "kasper/src/shell/utils/origin"

type DeclineInput struct {
	StoreId string `json:"storeId" validate:"required"`
}

func (d DeclineInput) GetData() any {
	return "dummy"
}

func (d DeclineInput) GetStoreId() string {
	return ""
}

func (d DeclineInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
