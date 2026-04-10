package inputs_stores

import "kasper/src/shell/utils/origin"

type GetInput struct {
	StoreId     string `json:"storeId" validate:"required"`
	IncludeMeta bool   `json:"includeMeta"`
}

func (d GetInput) GetData() any {
	return "dummy"
}

func (d GetInput) GetStoreId() string {
	return d.StoreId
}

func (d GetInput) Origin() string {
	o := origin.FindOriginLocal(d.StoreId)
	if o == "global" {
		return ""
	}
	return o
}
