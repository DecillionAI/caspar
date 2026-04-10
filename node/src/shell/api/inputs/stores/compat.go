package inputs_stores

type SignalInput struct {
	Type    string `json:"type"`
	StoreId string `json:"storeId"`
	UserId  string `json:"userId"`
	Data    string `json:"data"`
	Temp    bool   `json:"temp,omitempty"`
}

type JoinInput struct {
	StoreId string `json:"storeId"`
}
