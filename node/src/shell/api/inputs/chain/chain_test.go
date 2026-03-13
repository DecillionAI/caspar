package inputs_machiner

import (
	"testing"

	"kasper/src/abstract/models/input"
)

func TestChainInputsImplementInterface(t *testing.T) {
	var _ input.IInput = CreateInput{}
	var _ input.IInput = CreateFromPointInput{}
	var _ input.IInput = CreateShardInput{}
	var _ input.IInput = RegisterNodeInput{}
	var _ input.IInput = SubBaseTrxInput{}
}

func TestChainInputOriginsAndPointIds(t *testing.T) {
	point := "pt@fedorigin"
	if in := (CreateFromPointInput{PointId: point}); in.GetPointId() != point || in.Origin() != "fedorigin" {
		t.Fatalf("unexpected create_from_point values point=%q origin=%q", in.GetPointId(), in.Origin())
	}

	globalInputs := []input.IInput{CreateInput{}, RegisterNodeInput{}, CreateShardInput{}, SubBaseTrxInput{}}
	for _, in := range globalInputs {
		if in.GetPointId() != "" || in.Origin() != "global" {
			t.Fatalf("unexpected global input values point=%q origin=%q", in.GetPointId(), in.Origin())
		}
	}
}
