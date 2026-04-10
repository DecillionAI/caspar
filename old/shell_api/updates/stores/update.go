package updates_stores

import "kasper/src/shell/api/model"

type Update struct {
	Store model.Store `json:"store"`
}
