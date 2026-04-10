package actions_invite

import (
	"errors"
	"kasper/src/abstract/models/action"
	"kasper/src/abstract/models/core"
	"kasper/src/abstract/state"
	inputsinvites "kasper/src/shell/api/inputs/invites"
	"kasper/src/shell/api/model"
	outputsinvites "kasper/src/shell/api/outputs/invites"
	updatesinvites "kasper/src/shell/api/updates/invites"
	updates_stores "kasper/src/shell/api/updates/stores"
	"kasper/src/shell/utils/future"
	"log"
	"strconv"
	"time"
)

type Actions struct {
	App           core.ICore
	modelExtender map[string]map[string]action.ExtendedField
}

func Install(a *Actions, params ...any) error {
	if len(params) >= 1 {
		a.modelExtender = params[0].(map[string]map[string]action.ExtendedField)
	} else {
		a.modelExtender = map[string]map[string]action.ExtendedField{}
	}
	if _, ok := a.modelExtender["user"]; !ok {
		a.modelExtender["user"] = map[string]action.ExtendedField{}
	}
	return nil
}

// Create /invites/create check [ true true false ] access [ true false false false POST ]
func (a *Actions) Create(state state.IState, input inputsinvites.CreateInput) (any, error) {
	trx := state.Trx()
	if trx.GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		return nil, errors.New("you are not admin")
	}
	if !trx.HasObj("Store", state.Info().StoreId()) {
		return nil, errors.New("store not found")
	}
	store := model.Store{Id: state.Info().StoreId()}.Pull(trx)
	if store.Tag == "home" {
		return nil, errors.New("home is not extendable")
	}
	trx.PutLink("invite::"+store.Id+"::"+input.UserId, "true")
	trx.PutLink("inviteto::"+input.UserId+"::"+store.Id, "true")
	trx.PutLink("invitetime::"+store.Id+"::"+input.UserId, strconv.FormatInt(time.Now().UnixMilli(), 10))
	future.Async(func() {
		a.App.Tools().Signaler().SignalUser("invites/create", input.UserId, updatesinvites.Create{Store: store}, true)
	}, false)
	return outputsinvites.CreateOutput{}, nil
}

// ListStoreInvites /invites/listStoreInvites check [ true true false ] access [ true false false false POST ]
func (a *Actions) ListStoreInvites(state state.IState, input inputsinvites.ListStoreInvitesInput) (any, error) {
	trx := state.Trx()
	if trx.GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		return nil, errors.New("you are not admin")
	}
	if !trx.HasObj("Store", state.Info().StoreId()) {
		return nil, errors.New("store not found")
	}
	prefix := "invite::" + state.Info().StoreId() + "::"
	links, err := trx.GetLinksList(prefix, 0, 1000)
	if err != nil {
		log.Println(err)
		return nil, err
	}
	users := []map[string]any{}
	for _, link := range links {
		user := model.User{Id: link[len(prefix):]}.Pull(trx)
		t, _ := strconv.ParseInt(trx.GetLink("invitetime::"+state.Info().StoreId()+"::"+user.Id), 10, 64)
		result := map[string]any{
			"id":       user.Id,
			"username": user.Username,
			"type":     user.Typ,
			"time":     t,
		}
		for _, v := range a.modelExtender["user"] {
			if meta, err := trx.GetJson("UserMeta::"+user.Id, v.Path); err == nil || meta[v.Name] != nil {
				result[v.Name] = meta[v.Name]
			}
		}
		users = append(users, result)
	}
	return map[string]any{"users": users}, nil
}

// ListUserInvites /invites/listUserInvites check [ true false false ] access [ true false false false POST ]
func (a *Actions) ListUserInvites(state state.IState, input inputsinvites.ListUserInvitesInput) (any, error) {
	trx := state.Trx()
	prefix := "inviteto::" + state.Info().UserId() + "::"
	links, err := trx.GetLinksList(prefix, 0, 1000)
	if err != nil {
		log.Println(err)
		return nil, err
	}
	stores := []map[string]any{}
	for _, link := range links {
		store := model.Store{Id: link[len(prefix):]}.Pull(trx)
		t, _ := strconv.ParseInt(trx.GetLink("invitetime::"+store.Id+"::"+state.Info().UserId()), 10, 64)

		result := map[string]any{
			"id":          store.Id,
			"isPublic":    store.IsPublic,
			"persHist":    store.PersHist,
			"memberCount": store.MemberCount,
			"signalCount": store.SignalCount,
			"parentId":    store.ParentId,
			"tag":         store.Tag,
			"time":        t,
		}
		for _, v := range a.modelExtender["store"] {
			if meta, err := trx.GetJson("StoreMeta::"+store.Id, v.Path); err == nil || meta[v.Name] != nil {
				result[v.Name] = meta[v.Name]
			}
		}
		stores = append(stores, result)
	}
	return map[string]any{"stores": stores}, nil
}

// Cancel /invites/cancel check [ true true false ] access [ true false false false POST ]
func (a *Actions) Cancel(state state.IState, input inputsinvites.CancelInput) (any, error) {
	trx := state.Trx()
	if trx.GetLink("admin::"+state.Info().StoreId()+"::"+state.Info().UserId()) != "true" {
		return nil, errors.New("you are not admin")
	}
	if trx.GetLink("invite::"+state.Info().StoreId()+"::"+input.UserId) != "true" {
		return nil, errors.New("invitation does not exist")
	}
	trx.DelKey("link::invite::" + state.Info().StoreId() + "::" + input.UserId)
	trx.DelKey("link::inviteto::" + input.UserId + "::" + state.Info().StoreId())
	trx.DelKey("link::invitetime::" + state.Info().StoreId() + "::" + input.UserId)
	store := model.Store{Id: state.Info().StoreId()}.Pull(trx)
	future.Async(func() {
		a.App.Tools().Signaler().SignalUser("invites/cancel", input.UserId, updatesinvites.Cancel{Store: store}, true)
	}, false)
	return outputsinvites.CancelOutput{}, nil
}

// Accept /invites/accept check [ true false false ] access [ true false false false POST ]
func (a *Actions) Accept(state state.IState, input inputsinvites.AcceptInput) (any, error) {
	trx := state.Trx()
	if trx.GetLink("invite::"+input.StoreId+"::"+state.Info().UserId()) != "true" {
		return nil, errors.New("invitation does not exist")
	}
	trx.DelKey("link::invite::" + input.StoreId + "::" + state.Info().UserId())
	trx.DelKey("link::inviteto::" + state.Info().UserId() + "::" + input.StoreId)
	trx.DelKey("link::invitetime::" + input.StoreId + "::" + state.Info().UserId())
	trx.PutLink("onaccess::"+input.StoreId+"::"+state.Info().UserId(), "true")
	trx.PutLink("hasaccess::"+state.Info().UserId()+"::"+input.StoreId, "true")
	a.App.Tools().Signaler().JoinGroup(input.StoreId, state.Info().UserId())
	admins, err := trx.GetLinksList("admin::"+input.StoreId+"::", -1, -1)
	if err != nil {
		log.Println(err)
		return nil, err
	}
	user := model.User{Id: state.Info().UserId()}.Pull(trx)
	for _, admin := range admins {
		future.Async(func() {
			a.App.Tools().Signaler().SignalUser("invites/accept", admin, updatesinvites.Accept{User: user, StoreId: input.StoreId}, true)
		}, false)
	}
	future.Async(func() {
		a.App.Tools().Signaler().SignalGroup("spaces/userJoined", input.StoreId, updates_stores.Join{StoreId: input.StoreId, User: user}, true, []string{})
	}, false)
	return outputsinvites.AcceptOutput{}, nil
}

// Decline /invites/decline check [ true false false ] access [ true false false false POST ]
func (a *Actions) Decline(state state.IState, input inputsinvites.DeclineInput) (any, error) {
	trx := state.Trx()
	if trx.GetLink("invite::"+input.StoreId+"::"+state.Info().UserId()) != "true" {
		return nil, errors.New("invitation does not exist")
	}
	trx.DelKey("link::invite::" + input.StoreId + "::" + state.Info().UserId())
	trx.DelKey("link::inviteto::" + state.Info().UserId() + "::" + input.StoreId)
	trx.DelKey("link::invitetime::" + input.StoreId + "::" + state.Info().UserId())
	admins, err := trx.GetLinksList("admin::"+input.StoreId+"::", -1, -1)
	if err != nil {
		log.Println(err)
		return nil, err
	}
	user := model.User{Id: state.Info().UserId()}.Pull(trx)
	for _, admin := range admins {
		future.Async(func() {
			a.App.Tools().Signaler().SignalUser("invites/decline", admin, updatesinvites.Accept{User: user, StoreId: input.StoreId}, true)
		}, false)
	}
	return outputsinvites.DeclineOutput{}, nil
}
