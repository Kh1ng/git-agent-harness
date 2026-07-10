import { useState } from 'react';
import { Plus, Pencil, Trash2, Check, X, AlertCircle, Info } from 'lucide-react';
import { useGahStore } from '../store/gahStore.js';
import type { ProfileSummary } from '@git-agent-harness/contracts';
import type { ProfileAddData, ProfileUpdateData } from '../api/client.js';

export function ProfileEditor() {
  const {
    profiles,
    profileCrud,
    addProfile,
    updateProfile,
    removeProfile,
    fetchProfiles,
    clearProfileErrors
  } = useGahStore();

  const [editingProfile, setEditingProfile] = useState<string | null>(null);
  const [showAddForm, setShowAddForm] = useState(false);
  const [showDeleteConfirm, setShowDeleteConfirm] = useState<string | null>(null);
  
  const [formData, setFormData] = useState<Omit<ProfileAddData, 'name'> & { name?: string }>({
    display_name: '',
    repo_id: '',
    provider: 'github',
    repo: '',
    local_path: '',
    artifact_root: '',
    default_target_branch: 'main',
  });

  const errorMessage = profileCrud.addError || profileCrud.updateError || profileCrud.removeError;
  const showSuccess = profileCrud.lastAddSuccess || profileCrud.lastUpdateSuccess || profileCrud.lastRemoveSuccess;
  const successMessage = profileCrud.lastAddSuccess 
    ? 'Profile added successfully!'
    : profileCrud.lastUpdateSuccess
      ? 'Profile updated successfully!'
      : profileCrud.lastRemoveSuccess
        ? 'Profile removed successfully!'
        : '';

  const resetForm = () => {
    setFormData({
      display_name: '',
      repo_id: '',
      provider: 'github',
      repo: '',
      local_path: '',
      artifact_root: '',
      default_target_branch: 'main',
    });
    setEditingProfile(null);
  };

  const loadProfileForEdit = (profile: ProfileSummary) => {
    setFormData({
      display_name: profile.display_name,
      repo_id: '',
      provider: profile.provider,
      repo: profile.repo,
      local_path: profile.local_path,
      artifact_root: '',
      default_target_branch: 'main',
    });
    setEditingProfile(profile.name);
    setShowAddForm(true);
  };

  const handleChange = (field: keyof Omit<ProfileAddData, 'name'>, value: string) => {
    setFormData(prev => ({ ...prev, [field]: value }));
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    clearProfileErrors();
    
    try {
      const data: ProfileAddData = {
        name: editingProfile || formData.display_name.toLowerCase().replace(/\s+/g, '-'),
        display_name: formData.display_name,
        repo_id: formData.repo_id || formData.display_name.toLowerCase().replace(/\s+/g, '-'),
        provider: formData.provider || 'github',
        repo: formData.repo,
        local_path: formData.local_path,
        artifact_root: formData.artifact_root || formData.local_path,
        default_target_branch: formData.default_target_branch || 'main',
      };
      
      if (editingProfile) {
        const updateData: ProfileUpdateData = {
          display_name: formData.display_name,
          repo_id: formData.repo_id,
          provider: formData.provider,
          repo: formData.repo,
          local_path: formData.local_path,
          artifact_root: formData.artifact_root,
          default_target_branch: formData.default_target_branch,
        };
        await updateProfile(editingProfile, updateData);
      } else {
        await addProfile(data);
      }
      
      await fetchProfiles();
      resetForm();
      setShowAddForm(false);
    } catch (error) {
      console.error('Profile save error:', error);
    }
  };

  const handleDelete = async (profileName: string) => {
    clearProfileErrors();
    try {
      await removeProfile(profileName, { force: true });
      await fetchProfiles();
      setShowDeleteConfirm(null);
    } catch (error) {
      console.error('Profile delete error:', error);
    }
  };

  const profileList = profiles.data || [];
  const isLoading = profiles.loading || profileCrud.adding || profileCrud.updating || profileCrud.removing;

  return (
    <div className="space-y-4">
      <div className="flex justify-between items-center">
        <h3 className="text-sm font-semibold text-primary">Profile Management</h3>
        <button
          onClick={() => {
            resetForm();
            setShowAddForm(!showAddForm);
          }}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 bg-accent text-white rounded-md text-sm font-medium hover:bg-accent/90"
        >
          <Plus size={14} aria-hidden="true" />
          Add Profile
        </button>
      </div>

      {errorMessage && (
        <div className="p-3 bg-red-50 border border-red-200 text-red-700 text-sm rounded-md">
          <AlertCircle size={14} className="inline mr-1" aria-hidden="true" />
          {errorMessage}
        </div>
      )}
      {showSuccess && (
        <div className="p-3 bg-green-50 border border-green-200 text-green-700 text-sm rounded-md">
          <Check size={14} className="inline mr-1" aria-hidden="true" />
          {successMessage}
        </div>
      )}

      {showAddForm && (
        <ProfileForm
          formData={formData}
          editingProfile={editingProfile}
          isLoading={isLoading}
          onChange={handleChange}
          onSubmit={handleSubmit}
          onCancel={() => {
            resetForm();
            setShowAddForm(false);
          }}
        />
      )}

      <ProfileListComponent
        profileList={profileList}
        isLoading={isLoading}
        onEdit={loadProfileForEdit}
        onDelete={(name) => setShowDeleteConfirm(name)}
      />

      {showDeleteConfirm && (
        <DeleteModal
          profileName={showDeleteConfirm}
          isLoading={isLoading}
          onCancel={() => setShowDeleteConfirm(null)}
          onConfirm={() => handleDelete(showDeleteConfirm)}
        />
      )}
    </div>
  );
}

interface ProfileFormProps {
  formData: Omit<ProfileAddData, 'name'> & { name?: string };
  editingProfile: string | null;
  isLoading: boolean;
  onChange: (field: keyof Omit<ProfileAddData, 'name'>, value: string) => void;
  onSubmit: (e: React.FormEvent) => Promise<void>;
  onCancel: () => void;
}

function ProfileForm({ formData, editingProfile, isLoading, onChange, onSubmit, onCancel }: ProfileFormProps) {
  return (
    <form onSubmit={onSubmit} className="p-4 space-y-4 border border-subtle rounded-lg bg-raised">
      <h4 className="text-sm font-medium text-primary">
        {editingProfile ? `Edit Profile: ${editingProfile}` : 'Add New Profile'}
      </h4>
      
      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <div>
          <label className="block text-xs font-medium text-secondary mb-1">
            Display Name *
          </label>
          <input
            type="text"
            value={formData.display_name}
            onChange={(e) => onChange('display_name', e.target.value)}
            className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
            placeholder="My Project"
            required
          />
        </div>
        
        <div>
          <label className="block text-xs font-medium text-secondary mb-1">
            Profile Name (ID) *
          </label>
          <input
            type="text"
            value={editingProfile ? editingProfile : formData.repo_id}
            onChange={(e) => onChange('repo_id', e.target.value)}
            className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
            placeholder="my-project"
            disabled={!!editingProfile}
            required
          />
          <p className="text-xs text-muted mt-1">
            {editingProfile ? 'Profile name cannot be changed after creation' : 'Used as config key in TOML'}
          </p>
        </div>

        <div>
          <label className="block text-xs font-medium text-secondary mb-1">
            Provider *
          </label>
          <select
            value={formData.provider}
            onChange={(e) => onChange('provider', e.target.value)}
            className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
            required
          >
            <option value="github">GitHub</option>
            <option value="gitlab">GitLab</option>
          </select>
        </div>

        <div>
          <label className="block text-xs font-medium text-secondary mb-1">
            Repository *
          </label>
          <input
            type="text"
            value={formData.repo}
            onChange={(e) => onChange('repo', e.target.value)}
            className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
            placeholder="owner/repo"
            required
          />
        </div>

        <div>
          <label className="block text-xs font-medium text-secondary mb-1">
            Local Path *
          </label>
          <input
            type="text"
            value={formData.local_path}
            onChange={(e) => onChange('local_path', e.target.value)}
            className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
            placeholder="/path/to/local/repo"
            required
          />
        </div>

        <div>
          <label className="block text-xs font-medium text-secondary mb-1">
            Artifact Root *
          </label>
          <input
            type="text"
            value={formData.artifact_root}
            onChange={(e) => onChange('artifact_root', e.target.value)}
            className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
            placeholder="/path/to/artifacts"
            required
          />
        </div>

        <div className="md:col-span-2">
          <label className="block text-xs font-medium text-secondary mb-1">
            Default Branch
          </label>
          <input
            type="text"
            value={formData.default_target_branch}
            onChange={(e) => onChange('default_target_branch', e.target.value)}
            className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
            placeholder="main"
          />
        </div>
      </div>

      <p className="text-xs text-muted inline-flex items-start gap-1.5">
        <Info size={13} className="shrink-0 mt-0.5" aria-hidden="true" />
        Fields marked with * are required. For GitLab, you may need to set provider_api_base after creation.
      </p>

      <div className="flex gap-2">
        <button
          type="submit"
          disabled={isLoading}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 bg-accent text-white rounded-md text-sm font-medium hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          <Check size={14} aria-hidden="true" />
          {isLoading ? 'Saving...' : editingProfile ? 'Update Profile' : 'Add Profile'}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 bg-raised border border-subtle rounded-md text-sm text-secondary hover:bg-white/5"
        >
          <X size={14} aria-hidden="true" />
          Cancel
        </button>
      </div>
    </form>
  );
}

interface ProfileListComponentProps {
  profileList: ProfileSummary[];
  isLoading: boolean;
  onEdit: (profile: ProfileSummary) => void;
  onDelete: (name: string) => void;
}

function ProfileListComponent({ profileList, isLoading, onEdit, onDelete }: ProfileListComponentProps) {
  if (isLoading && !profileList.length) {
    return <p className="text-sm text-muted">Loading profiles...</p>;
  }
  
  if (profileList.length === 0) {
    return <p className="text-sm text-muted">No profiles found. Add your first profile above.</p>;
  }

  return (
    <div className="space-y-1">
      {profileList.map((profile) => (
        <div
          key={profile.name}
          className="flex items-center justify-between p-2 bg-raised border border-subtle rounded-md"
        >
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2">
              <div className="w-2 h-2 bg-accent rounded-full" />
              <div>
                <p className="text-sm font-medium text-primary">{profile.display_name}</p>
                <p className="text-xs text-muted">{profile.name} · {profile.provider} · {profile.repo}</p>
              </div>
            </div>
          </div>
          
          <div className="flex gap-1">
            <button
              onClick={() => onEdit(profile)}
              className="p-1.5 text-muted hover:text-primary hover:bg-white/5 rounded-md"
              title="Edit profile"
            >
              <Pencil size={14} aria-hidden="true" />
            </button>
            <button
              onClick={() => onDelete(profile.name)}
              className="p-1.5 text-muted hover:text-red-500 hover:bg-red-50/10 rounded-md"
              title="Delete profile"
            >
              <Trash2 size={14} aria-hidden="true" />
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}

interface DeleteModalProps {
  profileName: string;
  isLoading: boolean;
  onCancel: () => void;
  onConfirm: () => Promise<void>;
}

function DeleteModal({ profileName, isLoading, onCancel, onConfirm }: DeleteModalProps) {
  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
      <div className="bg-raised border border-subtle rounded-lg p-6 max-w-md w-full">
        <h3 className="text-lg font-semibold text-primary mb-2">Delete Profile</h3>
        <p className="text-sm text-secondary mb-4">
          Are you sure you want to delete the profile <strong>{profileName}</strong>?
          This action cannot be undone.
        </p>
        <div className="flex gap-2 justify-end">
          <button
            onClick={onCancel}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 bg-raised border border-subtle rounded-md text-sm text-secondary hover:bg-white/5"
          >
            <X size={14} aria-hidden="true" />
            Cancel
          </button>
          <button
            onClick={onConfirm}
            disabled={isLoading}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 bg-red-600 text-white rounded-md text-sm font-medium hover:bg-red-700 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            <Trash2 size={14} aria-hidden="true" />
            {isLoading ? 'Deleting...' : 'Delete Profile'}
          </button>
        </div>
      </div>
    </div>
  );
}
