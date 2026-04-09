package inputs_machiner

import "kasper/src/shell/utils/origin"

type CreateFromPointInput struct {
	PointId       string  `json:"pointId" validate:"required"`
	IsTemp        *bool   `json:"isTemp" validate:"required"`
	LockId        *string `json:"lockId"`
	LockSignature *string `json:"lockSignature"`
}

func (d CreateFromPointInput) GetData() any {
	return "dummy"
}

func (d CreateFromPointInput) GetPointId() string {
	return d.PointId
}

func (d CreateFromPointInput) Origin() string {
	return origin.FindOriginLocal(d.PointId)
}
