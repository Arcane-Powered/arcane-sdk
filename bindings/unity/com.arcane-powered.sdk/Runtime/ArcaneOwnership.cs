// SPDX-License-Identifier: MIT OR Apache-2.0
namespace ArcanePowered
{
    /// <summary>Ownership as of the last check.</summary>
    /// <remarks>
    /// <see cref="Owned"/> and <see cref="DrmDisabled"/> both mean
    /// <em>launch the game</em>. There is no "not owned" member: a title the
    /// account does not own fails <see cref="Arcane.Init()"/> with
    /// <see cref="ArcaneErrorCode.NotOwned"/> instead of building a client.
    /// </remarks>
    public enum ArcaneOwnership
    {
        /// <summary>No client yet — <see cref="Arcane.Init()"/> has not succeeded.</summary>
        NotInitialized = -1,

        /// <summary>A valid ticket for this game id, this account and this device.</summary>
        Owned = 0,

        /// <summary>DRM is off for this title; no ticket is required.</summary>
        DrmDisabled = 1,
    }
}
