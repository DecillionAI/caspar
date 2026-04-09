package inputs_pc

import (
	"testing"

	"kasper/src/abstract/models/input"
)

func TestPcInputsImplementInterfaceAndScope(t *testing.T) {
	var _ input.IInput = RunPcInput{}
	var _ input.IInput = ExecCommandInput{}

	all := []input.IInput{RunPcInput{}, ExecCommandInput{}}
	for _, in := range all {
		if in.GetPointId() != "" || in.Origin() != "" {
			t.Fatalf("unexpected pc input values point=%q origin=%q", in.GetPointId(), in.Origin())
		}
	}
}
