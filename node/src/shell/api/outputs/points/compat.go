package outputs_points

import model "kasper/src/shell/api/model"

type AdminPoiint struct {
	Point model.Point `json:"point"`
	Admin bool        `json:"admin"`
}

type CreateOutput struct {
	Point AdminPoiint `json:"point"`
}
