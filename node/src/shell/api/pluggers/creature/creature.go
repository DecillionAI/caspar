package plugger_creature

import (
	iaction "kasper/src/abstract/models/action"
	"kasper/src/abstract/models/core"
	actions "kasper/src/shell/api/actions/creature"
	"kasper/src/shell/utils"
)

type Plugger struct {
	Id      *string
	Actions *actions.Actions
	Core    core.ICore
}

func (c *Plugger) Create() iaction.IAction {
	return utils.ExtractSecureAction(c.Core, c.Actions.Create)
}
func (c *Plugger) Get() iaction.IAction  { return utils.ExtractSecureAction(c.Core, c.Actions.Get) }
func (c *Plugger) List() iaction.IAction { return utils.ExtractSecureAction(c.Core, c.Actions.List) }
func (c *Plugger) Transfer() iaction.IAction {
	return utils.ExtractSecureAction(c.Core, c.Actions.Transfer)
}
func (c *Plugger) Signal() iaction.IAction {
	return utils.ExtractSecureAction(c.Core, c.Actions.Signal)
}

func (c *Plugger) Install(a *actions.Actions, extra ...any) *Plugger {
	err := actions.Install(a, extra...)
	if err != nil {
		panic(err)
	}
	return c
}

func New(actions *actions.Actions, core core.ICore) *Plugger {
	id := "creature"
	return &Plugger{Id: &id, Actions: actions, Core: core}
}
