package updates_points

import "kasper/src/shell/api/model"

type Fn struct {
	UserId     string          `json:"userId"`
	Typ        string          `json:"type"`
	Username   string          `json:"username"`
	PublicKey  string          `json:"publicKey"`
	MachineId  string          `json:"machineId"`
	Runtime    string          `json:"runtime"`
	Path       string          `json:"path"`
	Comment    string          `json:"comment"`
	Identifier string          `json:"identifier"`
	Metadata   map[string]any  `json:"metadata"`
	Access     map[string]bool `json:"access"`
}

type AddApp struct {
	PointId  string         `json:"pointId"`
	Machine  model.Machine  `json:"machine"`
	Programs map[string]*Fn `json:"programs"`
}
