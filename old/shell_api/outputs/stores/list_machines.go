package outputs_stores

import (
	"kasper/src/shell/api/model"
	updates_stores "kasper/src/shell/api/updates/stores"
)

type ListStoreMachinesOutput struct {
	Programs map[string]*updates_stores.Fn `json:"programs"`
	Models   map[string]model.Machine      `json:"models"`
}
