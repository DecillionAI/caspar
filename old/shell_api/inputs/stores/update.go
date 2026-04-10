package inputs_stores

import "kasper/src/shell/utils/origin"

type UpdateInput struct {
	StoreId  string `json:"storeId" validate:"required"`
	IsPublic *bool  `json:"isPublic"`
	PersHist *bool  `json:"persHist"`
	Metadata any    `json:"metadata"`
}

func (d UpdateInput) GetData() any {
	return "dummy"
}

func (d UpdateInput) GetStoreId() string {
	return d.StoreId
}

func (d UpdateInput) Origin() string {
	return origin.FindOriginLocal(d.StoreId)
}
