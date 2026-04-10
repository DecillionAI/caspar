package updates_stores

import "kasper/src/shell/api/model"

type Join struct {
	StoreId string     `json:"storeId"`
	User    model.User `json:"user"`
}
