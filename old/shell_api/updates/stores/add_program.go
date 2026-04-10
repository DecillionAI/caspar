package updates_stores

import "kasper/src/shell/api/model"

type AddProgram struct {
	StoreId string        `json:"storeId"`
	Model   model.Machine `json:"program"`
	Program Fn            `json:"programData"`
}
