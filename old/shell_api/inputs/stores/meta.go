package inputs_stores

import "kasper/src/shell/utils/origin"

type MetaInput struct {
	StoreId string `json:"storeId" validate:"required"`
	Path    string `json:"path" validate:"required"`
}

func (d MetaInput) GetData() any {
	return "dummy"
}

func (d MetaInput) GetStoreId() string {
	return d.StoreId
}

func (d MetaInput) Origin() string {
	o := origin.FindOriginLocal(d.StoreId)
	if o == "global" {
		return ""
	}
	return o
}
