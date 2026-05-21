UPDATE apps
SET public_exposure = true
WHERE current_deployment_id IS NOT NULL;
