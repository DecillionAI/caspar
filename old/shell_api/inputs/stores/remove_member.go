package inputs_stores

import "kasper/src/shell/utils/origin"

type RemoveMemberInput struct {
	UserId  string `json:"userId" validate:"required"`
	StoreId string `json:"storeId" validate:"required"`
}

func (d RemoveMemberInput) GetData() any {
	return "dummy"
}

func (d RemoveMemberInput) GetStoreId() string {
	return d.StoreId
}

func (d RemoveMemberInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
