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
	var _ input.IInput = UploadPointEntityInput{}
	var _ input.IInput = DownloadPointEntityInput{}
	var _ input.IInput = DeletePointEntityInput{}
}

func TestStorageInputOriginsFollowPointId(t *testing.T) {
	point := "p1@remote-origin"
	tests := []input.IInput{
		UploadPointEntityInput{PointId: point},
		DeletePointEntityInput{PointId: point},
		UploadDataInput{PointId: point},
	}
	for i, tt := range tests {
		if got := tt.GetPointId(); got != point {
			t.Fatalf("test %d point mismatch got=%q want=%q", i, got, point)
		}
		if got := tt.Origin(); got != "remote-origin" {
			t.Fatalf("test %d origin mismatch got=%q want=remote-origin", i, got)
		}
	}

	emptyOrigin := []input.IInput{
		DownloadPointEntityInput{PointId: point}, UploadInput{PointId: point}, DownloadInput{PointId: point},
		StreamGetInput{PointId: point}, DownloadUserEntityInput{}, DownloadAppEntityInput{},
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
