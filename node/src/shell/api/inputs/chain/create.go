package inputs_machiner

type CreateInput struct {
	IsTemp        *bool   `json:"isTemp" validate:"required"`
	LockId        *string `json:"lockId"`
	LockSignature *string `json:"lockSignature"`
}

func (d CreateInput) GetData() any {
	return "dummy"
}

func (d CreateInput) GetPointId() string {
	return ""
}

func (d CreateInput) Origin() string {
	return "global"
}
