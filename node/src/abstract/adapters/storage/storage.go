package storage

import (
	"kasper/src/abstract/models/packet"
	"kasper/src/abstract/models/trx"

	"database/sql"
	"github.com/dgraph-io/badger"
)

type IStorage interface {
	StorageRoot() string
	KvDb() *badger.DB
	TsDb() *sql.DB
	GenId(t trx.ITrx, origin string) string
	LogTimeSieries(storeId string, userId string, data string, timeVal int64) packet.LogPacket
	UpdateLog(storeId string, userId string, signalId string, data string, timeVal int64) packet.LogPacket
	ReadStoreLogs(storeId string, beforeTime int64, count int) []packet.LogPacket
	PickStoreLogs(storeId string, ids []string) []packet.LogPacket
	LogBuild(buildId string, machineId string, data string) packet.BuildPacket
	ReadBuildLogs(buildId string, machineId string) []packet.BuildPacket
}
