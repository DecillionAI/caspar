package inputs_invites

import "testing"

func TestInviteInputsOriginAndPointAccessors(t *testing.T) {
	create := CreateInput{PointId: "point@origin-a", UserId: "u1"}
	if create.GetPointId() != "point@origin-a" || create.Origin() != "origin-a" {
		t.Fatalf("unexpected create accessors: point=%q origin=%q", create.GetPointId(), create.Origin())
	}

	cancel := CancelInput{PointId: "point@global", UserId: "u2"}
	if cancel.GetPointId() != "point@global" || cancel.Origin() != "" {
		t.Fatalf("unexpected cancel accessors: point=%q origin=%q", cancel.GetPointId(), cancel.Origin())
	}

	accept := AcceptInput{PointId: "point@origin-b"}
	if accept.GetPointId() != "" || accept.Origin() != "origin-b" {
		t.Fatalf("unexpected accept accessors: point=%q origin=%q", accept.GetPointId(), accept.Origin())
	}

	decline := DeclineInput{PointId: "point@origin-c"}
	if decline.GetPointId() != "" || decline.Origin() != "origin-c" {
		t.Fatalf("unexpected decline accessors: point=%q origin=%q", decline.GetPointId(), decline.Origin())
	}
}
