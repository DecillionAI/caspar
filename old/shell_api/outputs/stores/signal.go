package outputs_stores

import "kasper/src/abstract/models/packet"

type SignalOutput struct {
	Packet packet.LogPacket `json:"packet"`
	Passed bool             `json:"passed"`
}
