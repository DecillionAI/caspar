package inputs_stores

import "kasper/src/shell/utils/origin"

type UpdateMemberInput struct {
	UserId   string         `json:"memberId" validate:"required"`
	StoreId  string         `json:"storeId" validate:"required"`
	Metadata map[string]any `json:"metadata" validate:"required"`
}

func (d UpdateMemberInput) GetData() any {
	return "dummy"
}

func (d UpdateMemberInput) GetStoreId() string {
	return d.StoreId
}

func (d UpdateMemberInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
