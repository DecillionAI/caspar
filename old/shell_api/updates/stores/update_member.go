package updates_stores

import "kasper/src/shell/api/model"

type UpdateMember struct {
	StoreId  string         `json:"storeId"`
	User     model.User     `json:"user"`
	Metadata map[string]any `json:"metadata"`
}
