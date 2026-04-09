package inputs_creatures

type SignalInput struct {
	Type       string `json:"type" validate:"required"` // pvp | all
	Data       string `json:"data" validate:"required"`
	PointId    string `json:"pointId"`
	CreatureId string `json:"creatureId"`
	Temp       bool   `json:"temp"`
}

func (d SignalInput) GetData() any       { return "dummy" }
func (d SignalInput) GetPointId() string { return d.PointId }
func (d SignalInput) Origin() string     { return "" }
