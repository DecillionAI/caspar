package inputs_points

type SignalInput struct {
	Type    string `json:"type"`
	PointId string `json:"pointId"`
	UserId  string `json:"userId"`
	Data    string `json:"data"`
	Temp    bool   `json:"temp,omitempty"`
}

type JoinInput struct {
	PointId string `json:"pointId"`
}
