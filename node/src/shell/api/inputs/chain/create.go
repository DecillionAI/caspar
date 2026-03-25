package inputs_machiner

type CreateInput struct {
	PointId       *string `json:"pointId"`
	IsTemp        *bool   `json:"isTemp" validate:"required"`
	LockId        *string `json:"lockId"`
	LockSignature *string `json:"lockSignature"`
}

func (d CreateInput) GetData() any {
	return "dummy"
}

func (d CreateInput) GetPointId() string {
	if d.PointId != nil {
		return *d.PointId
	}
	return ""
}

func (d CreateInput) Origin() string {
	return "global"
}
