package inputs_machiner

type CreateShardInput struct {
	ChainId       string   `json:"chainId" validate:"required"`
	ShardChainId  *string  `json:"shardChainId"`
	Peers         []string `json:"peers"`
	LockId        *string  `json:"lockId"`
	LockSignature *string  `json:"lockSignature"`
}

func (d CreateShardInput) GetData() any {
	return "dummy"
}

func (d CreateShardInput) GetPointId() string {
	return ""
}

func (d CreateShardInput) Origin() string {
	return "global"
}
