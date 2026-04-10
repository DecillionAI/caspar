package updates_stores

import "kasper/src/shell/api/model"

type Delete struct {
	Store model.Store `json:"store"`
}
