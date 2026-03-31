package inputs_machiner

type DeleteProgramInput struct {
	ProgramId string `json:"programId" validate:"required"`
}

func (d DeleteProgramInput) GetData() any {
	return "dummy"
}

func (d DeleteProgramInput) GetPointId() string {
	return ""
}

func (d DeleteProgramInput) Origin() string {
	return "global"
}
