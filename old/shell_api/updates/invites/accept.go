package updates_invites

import "kasper/src/shell/api/model"

type Accept struct {
	User  model.User `json:"user"`
	StoreId string `json:"storeId"`
}
