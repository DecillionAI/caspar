package inputs_invites

import "testing"

func TestInviteInputsOriginAndStoreAccessors(t *testing.T) {
	create := CreateInput{StoreId: "store@origin-a", UserId: "u1"}
	if create.GetStoreId() != "store@origin-a" || create.Origin() != "origin-a" {
		t.Fatalf("unexpected create accessors: store=%q origin=%q", create.GetStoreId(), create.Origin())
	}

	cancel := CancelInput{StoreId: "store@global", UserId: "u2"}
	if cancel.GetStoreId() != "store@global" || cancel.Origin() != "" {
		t.Fatalf("unexpected cancel accessors: store=%q origin=%q", cancel.GetStoreId(), cancel.Origin())
	}

	accept := AcceptInput{StoreId: "store@origin-b"}
	if accept.GetStoreId() != "" || accept.Origin() != "origin-b" {
		t.Fatalf("unexpected accept accessors: store=%q origin=%q", accept.GetStoreId(), accept.Origin())
	}

	decline := DeclineInput{StoreId: "store@origin-c"}
	if decline.GetStoreId() != "" || decline.Origin() != "origin-c" {
		t.Fatalf("unexpected decline accessors: store=%q origin=%q", decline.GetStoreId(), decline.Origin())
	}
}
