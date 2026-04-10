package outputs_stores

import "kasper/src/abstract/models/packet"

type HistoryOutput struct {
	Packets []packet.LogPacket `json:"packets"`
}
