package inputs_machiner

import (
	"testing"

	"kasper/src/abstract/models/input"
)

func TestMachineInputsImplementInterface(t *testing.T) {
	var _ input.IInput = ListAppMachsInput{}
	var _ input.IInput = ReadBuildLogsInput{}
	var _ input.IInput = CreateAppInput{}
	var _ input.IInput = CreateMachineInput{}
	var _ input.IInput = DeleteAppInput{}
	var _ input.IInput = DeleteMachineInput{}
	var _ input.IInput = DeployInput{}
	var _ input.IInput = ListInput{}
	var _ input.IInput = MachineBuildsInput{}
	var _ input.IInput = RunMachineInput{}
	var _ input.IInput = SignalInput{}
	var _ input.IInput = UpdateAppInput{}
	var _ input.IInput = UpdateMachineInput{}
}

func TestMachineInputOriginsAndPointIds(t *testing.T) {
	cases := []struct {
		in   input.IInput
		orig string
	}{
		{ListAppMachsInput{}, ""},
		{ListInput{}, ""},
		{SignalInput{}, ""},
		{ReadBuildLogsInput{}, "global"},
		{CreateAppInput{}, "global"},
		{CreateMachineInput{}, "global"},
		{DeleteAppInput{}, "global"},
		{DeleteMachineInput{}, "global"},
		{DeployInput{}, "global"},
		{MachineBuildsInput{}, "global"},
		{RunMachineInput{}, "global"},
		{UpdateAppInput{}, "global"},
		{UpdateMachineInput{}, "global"},
	}
	for i, c := range cases {
		if c.in.GetPointId() != "" {
			t.Fatalf("case %d expected empty point id", i)
		}
		if c.in.Origin() != c.orig {
			t.Fatalf("case %d origin got=%q want=%q", i, c.in.Origin(), c.orig)
		}
	}
}
