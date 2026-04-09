package updates_points

import model "kasper/src/shell/api/model"

type Send struct {
	User   model.User  `json:"user"`
	Point  model.Point `json:"point,omitempty"`
	Action string      `json:"action"`
	Data   string      `json:"data"`
	IsTemp bool        `json:"isTemp,omitempty"`
}

type Update struct {
	Point model.Point `json:"point"`
}

type Delete struct {
	Point model.Point `json:"point"`
}

type AddMember struct {
	PointId string     `json:"pointId"`
	User    model.User `json:"user"`
}

type UpdateMember struct {
	PointId  string         `json:"pointId"`
	User     model.User     `json:"user"`
	Metadata map[string]any `json:"metadata"`
}

type Join struct {
	PointId string     `json:"pointId"`
	User    model.User `json:"user"`
}
