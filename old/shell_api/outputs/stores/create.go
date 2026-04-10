package outputs_stores

import (
	"kasper/src/shell/api/model"
)

type AdminPoiint struct {
	model.Store
	Admin bool `json:"admin"`
}

type CreateOutput struct {
	Store AdminPoiint `json:"store"`
}
