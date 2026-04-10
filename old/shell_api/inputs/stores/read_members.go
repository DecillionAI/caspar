package inputs_stores

import "kasper/src/shell/utils/origin"

type ReadMemberInput struct {
	StoreId string `json:"storeId" validate:"required"`
}

func (d ReadMemberInput) GetData() any {
	return "dummy"
}

func (d ReadMemberInput) GetStoreId() string {
	return d.StoreId
}

func (d ReadMemberInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
