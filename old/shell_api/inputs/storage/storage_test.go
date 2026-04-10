package inputs_storage

import (
	"testing"

	"kasper/src/abstract/models/input"
)

func TestStorageInputsImplementInterface(t *testing.T) {
	var _ input.IInput = UploadUserEntityInput{}
	var _ input.IInput = DownloadUserEntityInput{}
	var _ input.IInput = UploadDataInput{}
	var _ input.IInput = UploadInput{}
	var _ input.IInput = DownloadInput{}
	var _ input.IInput = DownloadAppEntityInput{}
	var _ input.IInput = DeleteUserEntityInput{}
	var _ input.IInput = StreamGetInput{}
	var _ input.IInput = UploadAppEntityInput{}
	var _ input.IInput = UploadStoreEntityInput{}
	var _ input.IInput = DownloadStoreEntityInput{}
	var _ input.IInput = DeleteStoreEntityInput{}
}

func TestStorageInputOriginsFollowStoreId(t *testing.T) {
	store := "p1@remote-origin"
	tests := []input.IInput{
		UploadStoreEntityInput{StoreId: store},
		DeleteStoreEntityInput{StoreId: store},
		UploadDataInput{StoreId: store},
	}
	for i, tt := range tests {
		if got := tt.GetStoreId(); got != store {
			t.Fatalf("test %d store mismatch got=%q want=%q", i, got, store)
		}
		if got := tt.Origin(); got != "remote-origin" {
			t.Fatalf("test %d origin mismatch got=%q want=remote-origin", i, got)
		}
	}

	emptyOrigin := []input.IInput{
		DownloadStoreEntityInput{StoreId: store}, UploadInput{StoreId: store}, DownloadInput{StoreId: store},
		StreamGetInput{StoreId: store}, DownloadUserEntityInput{}, DownloadAppEntityInput{},
	}
	for i, tt := range emptyOrigin {
		if tt.Origin() != "" {
			t.Fatalf("empty-origin case %d expected empty origin", i)
		}
	}

	globalOnly := []input.IInput{UploadUserEntityInput{}, DeleteUserEntityInput{}, UploadAppEntityInput{}}
	for i, tt := range globalOnly {
		if tt.Origin() != "global" {
			t.Fatalf("global case %d expected global origin", i)
		}
	}
}
