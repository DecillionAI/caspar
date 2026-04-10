package inputs_machiner

import (
	"testing"

	"kasper/src/abstract/models/input"
)

func TestChainInputsImplementInterface(t *testing.T) {
	var _ input.IInput = CreateInput{}
	var _ input.IInput = CreateFromStoreInput{}
	var _ input.IInput = CreateShardInput{}
	var _ input.IInput = RegisterNodeInput{}
	var _ input.IInput = SubBaseTrxInput{}
}

func TestChainInputOriginsAndStoreIds(t *testing.T) {
	store := "pt@fedorigin"
	if in := (CreateFromStoreInput{StoreId: store}); in.GetStoreId() != store || in.Origin() != "fedorigin" {
		t.Fatalf("unexpected create_from_store values store=%q origin=%q", in.GetStoreId(), in.Origin())
	}

	globalInputs := []input.IInput{CreateInput{}, RegisterNodeInput{}, CreateShardInput{}, SubBaseTrxInput{}}
	for _, in := range globalInputs {
		if in.GetStoreId() != "" || in.Origin() != "global" {
			t.Fatalf("unexpected global input values store=%q origin=%q", in.GetStoreId(), in.Origin())
		}
	}
}
