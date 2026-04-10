package inputs_stores

import "kasper/src/shell/utils/origin"

type UpdateMemberAccessInput struct {
	UserId  string          `json:"memberId" validate:"required"`
	StoreId string          `json:"storeId" validate:"required"`
	Access  map[string]bool `json:"access" validate:"required"`
}

func (d UpdateMemberAccessInput) GetData() any {
	return "dummy"
}

func (d UpdateMemberAccessInput) GetStoreId() string {
	return d.StoreId
}

func (d UpdateMemberAccessInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
