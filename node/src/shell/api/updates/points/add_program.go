package updates_points

import "kasper/src/shell/api/model"

type AddMachine struct {
	PointId string        `json:"pointId"`
	Machine model.Machine `json:"machine"`
	Program Fn            `json:"program"`
}
