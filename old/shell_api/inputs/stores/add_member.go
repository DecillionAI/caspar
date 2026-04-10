package inputs_stores

import "kasper/src/shell/utils/origin"

type AddMemberInput struct {
	UserId   string          `json:"userId" validate:"required"`
	StoreId  string          `json:"storeId" validate:"required"`
	Metadata map[string]any  `json:"metadata" validate:"required"`
	Access   map[string]bool `json:"access" validate:"required"`
}

func (d AddMemberInput) GetData() any {
	return "dummy"
}

func (d AddMemberInput) GetStoreId() string {
	return d.StoreId
}

func (d AddMemberInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
