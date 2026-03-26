package outputs_points

import (
	"kasper/src/shell/api/model"
	updates_points "kasper/src/shell/api/updates/points"
)

type ListPointAppsOutput struct {
	Programs map[string]*updates_points.Fn `json:"programs"`
	Machines map[string]model.Machine      `json:"machines"`
}
