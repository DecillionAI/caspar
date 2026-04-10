package inputs_stores

import (
	"testing"

	"kasper/src/abstract/models/input"
)

func TestStoresInputsImplementInterface(t *testing.T) {
	var _ input.IInput = UpdateMemberAccessInput{}
	var _ input.IInput = DeleteInput{}
	var _ input.IInput = HistoryInput{}
	var _ input.IInput = RemoveMemberInput{}
	var _ input.IInput = AddMachineInput{}
	var _ input.IInput = UpdateMemberInput{}
	var _ input.IInput = ListInput{}
	var _ input.IInput = JoinInput{}
	var _ input.IInput = MetaInput{}
	var _ input.IInput = RemoveMachineInput{}
	var _ input.IInput = SignalInput{}
	var _ input.IInput = GetDefaultAccessInput{}
	var _ input.IInput = AddProgramInput{}
	var _ input.IInput = ListStoreMachinesInput{}
	var _ input.IInput = GlobalHistoryInput{}
	var _ input.IInput = AddMemberInput{}
	var _ input.IInput = GetInput{}
	var _ input.IInput = CreateInput{}
	var _ input.IInput = ReadInput{}
	var _ input.IInput = UpdateProgramInput{}
	var _ input.IInput = UpdateProgramAccessInput{}
	var _ input.IInput = RemoveProgramInput{}
	var _ input.IInput = ReadMemberInput{}
	var _ input.IInput = UpdateInput{}
}

func TestStoresOriginDerivation(t *testing.T) {
	store := "store@origin-x"
	localOriginStoreInputs := []input.IInput{
		UpdateMemberAccessInput{StoreId: store}, DeleteInput{StoreId: store}, RemoveMemberInput{StoreId: store},
		AddMachineInput{StoreId: store}, UpdateMemberInput{StoreId: store}, JoinInput{StoreId: store},
		RemoveMachineInput{StoreId: store}, AddProgramInput{StoreId: store}, ListStoreMachinesInput{StoreId: store},
		AddMemberInput{StoreId: store}, GetInput{StoreId: store}, UpdateProgramInput{StoreId: store},
		UpdateProgramAccessInput{StoreId: store}, RemoveProgramInput{StoreId: store}, ReadMemberInput{StoreId: store},
		UpdateInput{StoreId: store},
	}
	for i, in := range localOriginStoreInputs {
		if in.GetStoreId() != store {
			t.Fatalf("store-based case %d got store=%q", i, in.GetStoreId())
		}
		if in.Origin() != "origin-x" {
			t.Fatalf("store-based case %d got origin=%q", i, in.Origin())
		}
	}

	if m := (MetaInput{StoreId: "store@global"}).Origin(); m != "" {
		t.Fatalf("meta global should map to empty, got %q", m)
	}
	if got := (CreateInput{Orig: "global"}).Origin(); got != "" {
		t.Fatalf("create origin global should map to empty, got %q", got)
	}
	if got := (CreateInput{Orig: "origin-y"}).Origin(); got != "origin-y" {
		t.Fatalf("create origin mismatch got %q", got)
	}
	if r := (ReadInput{Orig: "origin-z"}); r.GetStoreId() != "" || r.Origin() != "origin-z" {
		t.Fatalf("read input mismatch store=%q origin=%q", r.GetStoreId(), r.Origin())
	}
	if s := (SignalInput{StoreId: store}); s.Origin() != "" {
		t.Fatalf("signal origin should be empty, got %q", s.Origin())
	}
	if h := (HistoryInput{StoreId: store}); h.Origin() != "" {
		t.Fatalf("history origin should be empty, got %q", h.Origin())
	}
	if g := (GlobalHistoryInput{}).Origin(); g != "" {
		t.Fatalf("global history origin mismatch: %q", g)
	}
	if l := (ListInput{}).Origin(); l != "" {
		t.Fatalf("list origin mismatch: %q", l)
	}
	if d := (GetDefaultAccessInput{}).Origin(); d != "" {
		t.Fatalf("default access origin mismatch: %q", d)
	}
}
