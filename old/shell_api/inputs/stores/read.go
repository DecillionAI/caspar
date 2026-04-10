package inputs_stores

import "kasper/src/shell/utils/origin"

type ReadInput struct {
	Offset int64  `json:"offset"`
	Count  int64  `json:"count"`
	Tag    string `json:"tag"`
	Orig   string `json:"orig"`
}

func (d ReadInput) GetData() any {
	return "dummy"
}

func (d ReadInput) GetStoreId() string {
	return ""
}

func (d ReadInput) Origin() string {
	return origin.LocalOnly(d.Orig)
}
