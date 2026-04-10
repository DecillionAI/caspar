package inputs_invites

import "kasper/src/shell/utils/origin"

type ListStoreInvitesInput struct {
	StoreId string `json:"storeId" validate:"required"`
}

func (d ListStoreInvitesInput) GetData() any {
	return "dummy"
}

func (d ListStoreInvitesInput) GetStoreId() string {
	return d.StoreId
}

func (d ListStoreInvitesInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
